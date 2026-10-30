//! `file:` URIs, as the client names documents, and the paths they stand for.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use lsp_types::Uri;

/// The path a `file:` URI names, or `None` for any other scheme, such as an
/// unsaved editor's `untitled:`.
pub(crate) fn to_path(uri: &Uri) -> Option<PathBuf> {
    let rest = uri.as_str().strip_prefix("file://")?;
    // Skip any host, such as `localhost`, up to the path.
    let path = &rest[rest.find('/')?..];
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let decoded = decode(path)?;
    // `/c:/dir` on Windows is the path `c:/dir`.
    let bytes = decoded.as_bytes();
    if cfg!(windows) && bytes.len() >= 3 && bytes[0] == b'/' && bytes[2] == b':' {
        return Some(PathBuf::from(&decoded[1..]));
    }
    Some(PathBuf::from(decoded))
}

/// The `file:` URI of an absolute path.
pub(crate) fn from_path(path: &Path) -> Uri {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut uri = String::from("file://");
    if !text.starts_with('/') {
        uri.push('/');
    }
    for byte in text.bytes() {
        let keep = byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte);
        if keep {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    Uri::from_str(&uri).expect("a percent-encoded path is a valid URI")
}

fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let path = Path::new("/home/a b/ünï/src/main.inset");
        let uri = from_path(path);
        assert_eq!(
            uri.as_str(),
            "file:///home/a%20b/%C3%BCn%C3%AF/src/main.inset"
        );
        assert_eq!(to_path(&uri).as_deref(), Some(path));
        let host = Uri::from_str("file://localhost/x/y.inset").unwrap();
        assert_eq!(to_path(&host).as_deref(), Some(Path::new("/x/y.inset")));
        let untitled = Uri::from_str("untitled:Untitled-1").unwrap();
        assert_eq!(to_path(&untitled), None);
    }
}
