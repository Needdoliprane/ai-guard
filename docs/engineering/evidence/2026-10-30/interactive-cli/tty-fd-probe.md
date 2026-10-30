# Diagnostic TTY fd registration — 2026-10-30

## Result

The isolated process ran under a private PTY (`tty:true`), with Tokio I/O active. It did not read from stdin or change its terminal mode. The exact device pathname returned by `ttyname_r` was used internally but never printed.

- stdin is a TTY; its terminal session and foreground group match the process.
- `/dev/tty` alias: TTY, same session/foreground, but not the same file/device identity as stdin. Tokio `AsyncFd::new` registration failed with raw OS error **22 (`EINVAL`)**.
- `ttyname_r` device: TTY, same file and `rdev` as stdin, same session/foreground. The same Tokio `AsyncFd::new` registration succeeded.
- stdin descriptor flags were `2` before and after; unchanged.

This reproduces the alias-vs-device registration difference on this host. Tokio 1.54.1 deprecates `AsyncFd::new` in favor of `AsyncFd::register`; in this version, `new` is the public constructor that performs the I/O-driver registration being diagnosed. No engine, admission, worker, provider, or repository code was involved.

## Provenance

Temporary Rust probe only: `/tmp/ai-guard-interactive-tty-fd-probe/src/main.rs`, SHA-256 `d8b39a4cd023617bf3cdfc2b999954d7f639d016fb9700c4de7c044f2979f990`. It depends on Tokio 1.54.1 and libc 0.2.191; Cargo lock generated offline outside the repository. Run log: `/tmp/ai-guard-interactive-tty-fd-probe/run.log`.

Exact execution command, shell login disabled and `tty:true`:

```sh
cargo run --manifest-path /tmp/ai-guard-interactive-tty-fd-probe/Cargo.toml --locked --offline
```

PATH explicitly included `/usr/bin`. No secrets, environment dump, terminal path, or input bytes were emitted. No product source, repository Cargo metadata, runtime state, or credentials were changed.
