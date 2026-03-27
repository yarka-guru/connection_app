# Privacy Policy

**ConnectionApp** does not collect, store, or transmit any personal data.

## What the app does

- Reads AWS configuration files from your local machine (`~/.aws/config`)
- Reads project configuration from `~/.connection-app/projects.json`
- Connects to AWS services (STS, EC2, RDS, SSM, Secrets Manager, SSO OIDC) using your existing AWS credentials
- Opens local TCP ports on your machine for port forwarding through AWS SSM

## Data handling

- **No telemetry**: The app does not send usage data, crash reports, or analytics to any server.
- **No accounts**: The app does not require registration or sign-up.
- **Local only**: All configuration and credentials remain on your machine.
- **AWS credentials**: Your AWS credentials are used exclusively to authenticate with AWS services. They are never sent to any third party.

## Third-party services

The app communicates only with AWS APIs using your configured AWS credentials. No other third-party services are contacted.

## Contact

If you have questions about this privacy policy, please open an issue on the [GitHub repository](https://github.com/yarka-guru/connection_app).
