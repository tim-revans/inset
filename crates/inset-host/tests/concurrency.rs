//! Concurrent requests, compiled from `tests/programs/relay`. A request
//! reserves the components it can reach, and requests whose reservations
//! do not overlap run side by side while one waits on a call to another
//! service. The others wait their turn.

mod common;

use common::{compile, fake};
use inset_host::{Completion, Incoming, Progress, Request, Response, Running};

fn get(path: &str) -> Request {
    Request {
        method: "GET".into(),
        path: path.into(),
        body: Vec::new(),
    }
}

fn put(path: &str, body: &str) -> Request {
    Request {
        method: "PUT".into(),
        path: path.into(),
        body: body.as_bytes().to_vec(),
    }
}

/// The replies in a progress, as status and text, by request ID.
fn replies(progress: &Progress) -> Vec<(u64, u16, String)> {
    progress
        .replies
        .iter()
        .map(|(id, Response { status, body, .. })| {
            (*id, *status, String::from_utf8(body.clone()).unwrap())
        })
        .collect()
}

fn deliver(running: &mut Running<'_>, request: &Request) -> (u64, Progress) {
    running
        .deliver("http", request)
        .expect("the program is running")
}

#[test]
fn a_request_that_shares_nothing_finishes_while_another_waits() {
    let program = compile("crates/inset-host/tests/programs/relay");
    let mut running = program.start(&fake()).unwrap();

    // The forecast waits on the forecast service.
    let (forecast, progress) = deliver(&mut running, &get("/forecast/London"));
    assert_eq!(replies(&progress), []);
    let [call] = progress.calls.as_slice() else {
        panic!("one call, not {:?}", progress.calls);
    };
    assert_eq!(
        (
            call.grant.as_str(),
            call.method.as_str(),
            call.path.as_str()
        ),
        ("weather", "GET", "/forecast/London")
    );
    let call = call.id;

    // The notebook and the front share nothing with it, so they answer now.
    let (note, progress) = deliver(&mut running, &put("/notes/plan", "rain"));
    assert_eq!(replies(&progress), [(note, 200, "kept".into())]);
    let (health, progress) = deliver(&mut running, &get("/health"));
    assert_eq!(replies(&progress), [(health, 200, "up".into())]);

    // Counting the forecasts reaches the forecasts desk, so it waits.
    let (asked, progress) = deliver(&mut running, &get("/asked"));
    assert_eq!(replies(&progress), []);

    // When the forecast comes, both finish, in the order they arrived.
    let progress = running
        .resume(vec![Completion {
            id: call,
            outcome: Ok(Incoming {
                status: 200,
                body: b"sunny".to_vec(),
            }),
        }])
        .unwrap();
    assert_eq!(
        replies(&progress),
        [(forecast, 200, "sunny".into()), (asked, 200, "1".into())]
    );
}

#[test]
fn requests_that_share_a_component_take_turns_in_order() {
    let program = compile("crates/inset-host/tests/programs/relay");
    let mut running = program.start(&fake()).unwrap();
    let (first, progress) = deliver(&mut running, &get("/forecast/London"));
    let first_call = progress.calls[0].id;
    // The second forecast waits for the first to finish before it starts,
    // so it makes no call yet.
    let (second, progress) = deliver(&mut running, &get("/forecast/Paris"));
    assert_eq!(progress, Progress::default());

    let answer = |id, body: &str| Completion {
        id,
        outcome: Ok(Incoming {
            status: 200,
            body: body.as_bytes().to_vec(),
        }),
    };
    let progress = running.resume(vec![answer(first_call, "sunny")]).unwrap();
    assert_eq!(replies(&progress), [(first, 200, "sunny".into())]);
    let [call] = progress.calls.as_slice() else {
        panic!("the second forecast's call, not {:?}", progress.calls);
    };
    assert_eq!(call.path, "/forecast/Paris");
    let progress = running.resume(vec![answer(call.id, "rain")]).unwrap();
    assert_eq!(replies(&progress), [(second, 200, "rain".into())]);
    let (_, progress) = deliver(&mut running, &get("/asked"));
    assert_eq!(replies(&progress)[0].2, "2");
}

#[test]
fn a_call_that_gets_no_reply_fails_only_its_request() {
    let program = compile("crates/inset-host/tests/programs/relay");
    let mut running = program.start(&fake()).unwrap();
    let (forecast, progress) = deliver(&mut running, &get("/forecast/London"));
    let call = progress.calls[0].id;
    let progress = running
        .resume(vec![Completion {
            id: call,
            outcome: Err("connection refused".into()),
        }])
        .unwrap();
    // `Capability(weather) => rollback`: the count is as it was.
    assert_eq!(replies(&progress), [(forecast, 500, String::new())]);
    let (_, progress) = deliver(&mut running, &get("/asked"));
    assert_eq!(replies(&progress)[0].2, "0");
    assert_eq!(running.reports().len(), 1);
    assert_eq!(running.reports()[0].failure, "Capability(weather)");
}

/// Sends a request over TCP and returns the raw reply.
fn http_get(port: u16, path: &str) -> String {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    reply
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// `inset run` with a real forecast service that is slow to answer: the
/// relay serves an unrelated route meanwhile, and relays the forecast when
/// it comes.
#[test]
fn a_slow_service_does_not_hold_up_an_unrelated_route() {
    use std::io::{Read, Write};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    // A forecast service that answers once it is told to.
    let upstream = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let upstream_port = upstream.local_addr().unwrap().port();
    let (release, released) = mpsc::channel::<()>();
    let (called, calls) = mpsc::channel::<()>();
    std::thread::spawn(move || {
        let (mut stream, _) = upstream.accept().unwrap();
        let mut request = [0; 1024];
        let _ = stream.read(&mut request).unwrap();
        called.send(()).unwrap();
        released.recv().unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nsunny")
            .unwrap();
    });

    let program: &'static inset_host::Program =
        Box::leak(Box::new(compile("crates/inset-host/tests/programs/relay")));
    let port = free_port();
    let dir = std::env::temp_dir().join(format!("inset-relay-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let options = inset_host::Options {
        settings: vec![
            ("port".into(), port.to_string()),
            (
                "forecasts".into(),
                format!("http://127.0.0.1:{upstream_port}"),
            ),
        ],
        mode: inset_host::Mode::Real,
        working_dir: dir.clone(),
    };
    let running = program.start(&options).unwrap();
    std::thread::spawn(move || inset_host::serve(running));

    // Wait until the relay listens.
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline, "the relay never listened");
        std::thread::sleep(Duration::from_millis(20));
    }
    let forecast = std::thread::spawn(move || http_get(port, "/forecast/London"));
    // Once the forecast waits on the service, ask for something else.
    calls.recv_timeout(Duration::from_secs(30)).unwrap();
    let health = http_get(port, "/health");
    // The forecast still waits on the service, and the health check is done.
    assert!(health.ends_with("\r\n\r\nup"), "{health}");
    assert!(!forecast.is_finished());
    release.send(()).unwrap();
    let forecast = forecast.join().unwrap();
    assert!(forecast.starts_with("HTTP/1.1 200 OK\r\n"), "{forecast}");
    assert!(forecast.ends_with("\r\n\r\nsunny"), "{forecast}");
    let _ = std::fs::remove_dir_all(&dir);
}
