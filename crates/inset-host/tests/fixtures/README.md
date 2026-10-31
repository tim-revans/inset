# Host test fixtures

Programs written by hand in the shape the Inset compiler emits, which the host's tests build to WebAssembly components and run. They do what no Inset program can, to show that the host refuses it. Programs compiled from Inset source are in `../programs/`.

| Fixture | Shows |
| --- | --- |
| `sandbox` | A program that reaches past its grants, as Rust in an external component might, for files, the network and a table it was not granted. Each is refused when it is called |
| `web-client` | A program that imports `wasi:http/outgoing-handler`, which the host never links, so it fails to instantiate |
