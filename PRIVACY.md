# Privacy Policy

**Effective 16 August 2026.** Applies to the ConnectionApp desktop app and
`connection-app-cli`, however you installed them (GitHub Releases, Homebrew,
or an app store).

The canonical version of this policy is published at
<https://yarka.guru/connection-app/privacy/>. This file mirrors it; every
change is recorded in git history.

**ConnectionApp does not collect, store, or transmit any personal data.**
There is no telemetry, no analytics, no crash reporting, and no account.
Everything the app knows lives on your computer; the only servers it talks
to are the AWS APIs in *your* AWS account and, for the update check, GitHub.

## What the app does

ConnectionApp opens port-forwarding tunnels through AWS Systems Manager (SSM)
so you can reach private RDS databases and other private services (VNC, RDP,
SSH) from your machine. To do that it:

- reads your AWS profiles from `~/.aws/config`;
- reads your project definitions from `~/.connection-app/projects.json`;
- calls AWS services in your account — STS, EC2, RDS, SSM, Secrets Manager
  and SSO OIDC — with your existing AWS credentials;
- opens a local TCP port on your machine and forwards it through an SSM
  session to the target.

## Data on your machine

The app writes only configuration and history that make it useful the next
time you open it. None of it is sent anywhere.

| Where | What |
|---|---|
| `~/.connection-app/projects.json` | Your project definitions (regions, RDS patterns, ports). You create and edit these. |
| `~/.connection-app/preferences.json` | UI preferences. |
| `~/.connection-app/history.jsonl` | A local log of connection events: timestamp, event type, project, AWS profile, and a short detail such as the local port or a retry count. No credentials, no query contents. |
| App data folder (`connections.json`) | Saved connections — the project/profile pairs you bookmarked, their groups and last-used times. On macOS this is `~/Library/Application Support/com.connection-app.desktop`. |
| `~/.aws/sso/cache/` | AWS SSO access tokens, in the same location and format the AWS CLI uses, so one sign-in serves both. Tokens expire on the schedule set by your AWS SSO administrator. |

Sandboxed builds (for example a Mac App Store build) cannot see `~/.aws` on
their own; on first launch you pick the folder once and the app keeps a
security-scoped bookmark to it (`sandbox.json` in the app data folder). You
can revoke that at any time by deleting the app.

## Credentials

- **AWS credentials** are used only to authenticate to AWS services in your
  account. They are never sent to any third party and never leave the
  standard AWS SDK/CLI locations.
- **Database passwords** are fetched per connection from AWS Secrets Manager
  — or generated as short-lived RDS IAM authentication tokens — and are held
  in memory only. The app never writes them to disk. If you click *Copy*, the
  password goes to your system clipboard until you copy something else.
- **SSH keys** stay where you keep them. For SSH tunnels the app only uses
  the key path you configured to print a ready-made `ssh -i …` command; it
  never reads the key itself.

## What leaves your machine

| Destination | When | What is sent |
|---|---|---|
| AWS APIs in your account (`*.amazonaws.com`) | When you sign in or connect | Ordinary AWS API requests signed with your credentials, and the SSM tunnel traffic itself, all over TLS. |
| Your AWS SSO start URL | When an SSO session needs renewing | The standard OIDC device-authorization flow, opened in your browser. |
| GitHub (`github.com`) | Desktop app from GitHub Releases or Homebrew: at launch and when you check for updates | A request for `latest.json` in the release feed, and the signed update if you accept it. Nothing beyond what any HTTPS request carries. Builds without the updater (app-store builds, and the CLI) never contact GitHub. |

That is the complete list. No third-party services, no advertising or
analytics SDKs, no crash reporters.

## Deleting your data

Delete `~/.connection-app` and the app data folder listed above; uninstall
the app. Homebrew users can run `brew uninstall --zap --cask connection-app`.
AWS SSO tokens in `~/.aws/sso/cache` are shared with the AWS CLI — remove
them with `aws sso logout` if you no longer want them.

## Changes to this policy

<https://yarka.guru/connection-app/privacy/> is the canonical version; this
file ships the same text in the repository, where every change is recorded in
git history. Substantive changes will be noted in the release notes.

## Contact

Questions about this policy: open an issue on the
[GitHub repository](https://github.com/yarka-guru/connection_app/issues).
Security reports: see <https://yarka.guru/.well-known/security.txt>.
Publisher: Yarka.Guru LLC — <https://yarka.guru/>.
