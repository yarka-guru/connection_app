# Third-Party Licenses

ConnectionApp includes the following open-source libraries. Their use is gratefully acknowledged.

## Apache-2.0

The following libraries are licensed under the Apache License, Version 2.0.
You may obtain a copy of the License at: https://www.apache.org/licenses/LICENSE-2.0

- **AWS SDK for Rust** — aws-config, aws-credential-types, aws-sdk-ec2, aws-sdk-ecs, aws-sdk-rds, aws-sdk-secretsmanager, aws-sdk-ssm, aws-sdk-ssooidc, aws-sdk-sts, aws-sigv4, aws-smithy-http-client
  Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.

## Apache-2.0 OR MIT (dual-licensed)

The following libraries are dual-licensed under Apache-2.0 or MIT. This project uses them under the MIT license.

- **Tauri** — tauri, tauri-build, tauri-plugin-dialog, tauri-plugin-notification, tauri-plugin-opener, tauri-plugin-store, tauri-plugin-updater
  Copyright The Tauri Programme within The Commons Conservancy.

- **Rust ecosystem** — base64, chrono, clap, dirs, env_logger, futures-util, log, regex, reqwest, semver, serde, serde_json, sha1, sha2, socket2, thiserror, uuid

## MIT

The following libraries are licensed under the MIT License.

- **Tokio** — tokio, tokio-util
  Copyright Tokio Contributors.

- **tokio-tungstenite**
  Copyright Daniel Abramov and contributors.

- **dialoguer**
  Copyright Armin Ronacher and contributors.

- **byteorder** (Unlicense OR MIT)
  Copyright Andrew Gallant.

## LGPL — bundled inside the Linux AppImage only

The Linux `.AppImage` is self-contained: it ships roughly 200 shared libraries
so it can run without system packages. Several of them are covered by the GNU
Lesser General Public License. The most significant:

- **WebKitGTK** — `libwebkit2gtk-4.1`, `libjavascriptcoregtk-4.1` (LGPL-2.1)
- **GTK 3** — `libgtk-3`, `libgdk-3` (LGPL-2.1)
- **GLib** — `libglib-2.0`, `libgio-2.0`, `libgobject-2.0` (LGPL-2.1)
- **libsoup** — `libsoup-3.0` (LGPL-2.0)
- **GnuTLS** — `libgnutls`, and `libnettle` / `libhogweed` (LGPL-2.1 / LGPL-3)
- **libgcrypt**, **libp11-kit**, **libtasn1**, **libidn2**, **libunistring** (LGPL)
- **GStreamer** core — `libgstreamer-1.0` (LGPL-2.1)
- **systemd** libraries — `libsystemd`, `libudev` (LGPL-2.1)

The full text of the LGPL version 2.1 is included in this repository at
[`licenses/LGPL-2.1.txt`](licenses/LGPL-2.1.txt). Components licensed under
LGPL-3 are available under the terms published at
<https://www.gnu.org/licenses/lgpl-3.0.txt>.

These libraries are **dynamically linked** and shipped as separate `.so` files
inside the AppImage. They are not modified, and they can be replaced with other
compatible versions by extracting the AppImage (`--appimage-extract`),
substituting the library, and repacking — which is what LGPL-2.1 §6 requires of
a work that uses the library.

**This section applies only to the `.AppImage`.** The `.deb` and `.rpm` packages
declare these components as system dependencies (`libwebkit2gtk-4.1-0`,
`libgtk-3-0`, `libappindicator3-1`) and do not redistribute them. The macOS
build uses Apple's own WKWebView system framework and bundles none of the above.

## Frontend (build-time only, not distributed)

The following are used as build tools and are not included in the distributed binary:

- Svelte (MIT)
- Vite (MIT)
- @tauri-apps/api (MIT)
- @tauri-apps/cli (MIT)
- @biomejs/biome (MIT)
- @sveltejs/vite-plugin-svelte (MIT)
