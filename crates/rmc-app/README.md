# rmc-app

`rmc-app` is the Rust cross-platform client for RustMediaCenter.

## Targets

- Desktop: Dioxus desktop
- Android: Dioxus mobile
- iOS: Dioxus mobile

## Development

Run desktop checks:

```bash
cargo check -p rmc-app
cargo test -p rmc-app
```

Run the desktop app:

```bash
cargo run -p rmc-app
```

Check mobile feature compilation on the host:

```bash
cargo check -p rmc-app --no-default-features --features mobile
```

## Playback contract

Direct playback uses the rmc-server endpoint below as the media element source:

```text
{server_base_url}/api/v1/movies/{play_id}/direct
```

The client does not pre-resolve 302 responses. The URL is assigned directly to
the HTML `video` element so the platform WebView/media stack follows the
server-provided `Location` header. If direct playback fails, use the transcode
mode endpoint:

```text
{server_base_url}/api/v1/movies/{play_id}/stream.mp4
```

For Android devices, do not leave the default `http://127.0.0.1:19000` unless
rmc-server runs inside the same device/emulator network namespace. Use the LAN
address of the server, and ensure the generated Android manifest allows
cleartext HTTP traffic or serve rmc-server behind HTTPS.

Android and iOS packaging require Dioxus CLI plus platform SDKs. If `dx` is not installed, install it before package validation:

```bash
cargo install dioxus-cli --locked
```

Android packaging requires Android SDK and NDK. iOS packaging requires macOS, Xcode, and iOS Rust targets. Do not report mobile packaging as verified unless those commands have run on a machine with the required platform toolchain.
