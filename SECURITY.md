# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 3.x     | :white_check_mark: |
| < 3.0   | :x:                |

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues.**

Instead, report vulnerabilities through [GitHub's private security advisory feature](https://github.com/yarka-guru/connection_app/security/advisories/new).

You should receive an acknowledgement within 48 hours. We will provide an initial assessment within one week and work with you to understand and address the issue.

Please include:

- Description of the vulnerability
- Steps to reproduce
- Potential impact
- Suggested fix (if any)

## Security Measures

- **No credentials stored locally** — database credentials are fetched from AWS Secrets Manager on each connection and never persisted to disk
- **Native tunneling** — SSM port forwarding is implemented directly over WebSocket with SigV4-signed connections; no external plugins required
- **SSO support** — AWS SSO tokens are cached in the standard AWS CLI location (`~/.aws/sso/cache/`) with `0600` permissions
- **macOS App Sandbox** — the desktop app supports App Store sandboxing via security-scoped bookmarks
- **No shell execution** — the app does not shell out to external processes for AWS operations
