import { EC2Client } from '@aws-sdk/client-ec2'
import { RDSClient } from '@aws-sdk/client-rds'
import { SSMClient } from '@aws-sdk/client-ssm'
import { STSClient } from '@aws-sdk/client-sts'
import { SecretsManagerClient } from '@aws-sdk/client-secrets-manager'
import { resolveCredentials } from './credential-resolver.js'

export function createAwsClients(profile, region, options = {}) {
  const credentials = resolveCredentials(profile, options)
  const config = { region, credentials }

  return {
    sts: new STSClient(config),
    secretsManager: new SecretsManagerClient(config),
    ec2: new EC2Client(config),
    rds: new RDSClient(config),
    ssm: new SSMClient(config),
  }
}

export function destroyAwsClients(clients) {
  for (const client of Object.values(clients)) {
    client.destroy()
  }
}
