import { GetCallerIdentityCommand } from '@aws-sdk/client-sts'
import {
  ListSecretsCommand,
  GetSecretValueCommand,
} from '@aws-sdk/client-secrets-manager'
import {
  DescribeInstancesCommand,
  TerminateInstancesCommand,
} from '@aws-sdk/client-ec2'
import {
  DescribeDBClustersCommand,
  DescribeDBInstancesCommand,
} from '@aws-sdk/client-rds'
import {
  DescribeInstanceInformationCommand,
  StartSessionCommand,
} from '@aws-sdk/client-ssm'

const DEFAULT_CREDENTIAL_TIMEOUT_MS = 15000

export async function checkCredentialsValid(clients) {
  const controller = new AbortController()
  const timeoutId = setTimeout(
    () => controller.abort(),
    DEFAULT_CREDENTIAL_TIMEOUT_MS,
  )

  try {
    const response = await clients.sts.send(
      new GetCallerIdentityCommand({}),
      { abortSignal: controller.signal },
    )
    clearTimeout(timeoutId)
    return {
      valid: true,
      identity: {
        account: response.Account,
        arn: response.Arn,
        userId: response.UserId,
      },
    }
  } catch (error) {
    clearTimeout(timeoutId)
    return {
      valid: false,
      error: error.name === 'AbortError'
        ? 'Credential check timed out after 15 seconds'
        : error.message,
    }
  }
}

export async function getConnectionCredentials(clients, secretPrefix, database) {
  const listResponse = await clients.secretsManager.send(
    new ListSecretsCommand({
      Filters: [{ Key: 'name', Values: [secretPrefix] }],
    }),
  )

  const secrets = listResponse.SecretList
  if (!secrets || secrets.length === 0) {
    throw new Error(
      `No secret found matching prefix '${secretPrefix}'.`,
    )
  }

  const secretName = secrets[0].Name

  const getResponse = await clients.secretsManager.send(
    new GetSecretValueCommand({ SecretId: secretName }),
  )

  if (!getResponse.SecretString) {
    throw new Error(
      `Secret '${secretName}' has no SecretString value.`,
    )
  }

  let credentials
  try {
    credentials = JSON.parse(getResponse.SecretString)
  } catch (error) {
    throw new Error(
      `Failed to parse credentials from secret '${secretName}': ${error.message}`,
    )
  }

  if (!credentials.username || !credentials.password) {
    throw new Error(
      `Secret '${secretName}' is missing required fields: username and/or password.`,
    )
  }

  return {
    username: credentials.username,
    password: credentials.password,
    database,
    secretName,
  }
}

export async function findBastionInstance(clients) {
  const response = await clients.ec2.send(
    new DescribeInstancesCommand({
      Filters: [
        { Name: 'tag:Name', Values: ['*bastion*'] },
        { Name: 'instance-state-name', Values: ['running'] },
      ],
    }),
  )

  const reservations = response.Reservations || []
  for (const reservation of reservations) {
    const instances = reservation.Instances || []
    if (instances.length > 0) {
      return instances[0].InstanceId
    }
  }

  throw new Error(
    'No running bastion instance found with tag Name=*bastion*.',
  )
}

export async function getRdsEndpoint(clients, rdsType, rdsPattern) {
  if (rdsType === 'cluster') {
    const response = await clients.rds.send(
      new DescribeDBClustersCommand({}),
    )

    const clusters = response.DBClusters || []
    const cluster = clusters.find(
      (c) =>
        c.Status === 'available' &&
        c.DBClusterIdentifier.endsWith(rdsPattern),
    )

    return cluster ? cluster.Endpoint : null
  }

  if (rdsType === 'instance') {
    const response = await clients.rds.send(
      new DescribeDBInstancesCommand({}),
    )

    const instances = response.DBInstances || []
    const instance = instances.find(
      (i) =>
        i.DBInstanceStatus === 'available' &&
        i.DBInstanceIdentifier.includes(rdsPattern),
    )

    return instance ? instance.Endpoint.Address : null
  }

  return null
}

export async function getRdsPort(clients, rdsType, rdsPattern, fallbackPort) {
  if (rdsType === 'cluster') {
    const response = await clients.rds.send(
      new DescribeDBClustersCommand({}),
    )

    const clusters = response.DBClusters || []
    const cluster = clusters.find(
      (c) =>
        c.Status === 'available' &&
        c.DBClusterIdentifier.endsWith(rdsPattern),
    )

    return cluster ? cluster.Port : fallbackPort
  }

  if (rdsType === 'instance') {
    const response = await clients.rds.send(
      new DescribeDBInstancesCommand({}),
    )

    const instances = response.DBInstances || []
    const instance = instances.find(
      (i) =>
        i.DBInstanceStatus === 'available' &&
        i.DBInstanceIdentifier.includes(rdsPattern),
    )

    return instance ? instance.Endpoint.Port : fallbackPort
  }

  return fallbackPort
}

export async function terminateBastionInstance(clients, instanceId) {
  await clients.ec2.send(
    new TerminateInstancesCommand({
      InstanceIds: [instanceId],
    }),
  )
}

const defaultSleepFn = (ms) => new Promise((r) => setTimeout(r, ms))

export async function waitForNewBastionInstance(
  clients,
  oldInstanceId,
  maxRetries,
  retryDelayMs,
  sleepFn = defaultSleepFn,
) {
  for (let attempt = 1; attempt <= maxRetries; attempt++) {
    const response = await clients.ec2.send(
      new DescribeInstancesCommand({
        Filters: [
          { Name: 'tag:Name', Values: ['*bastion*'] },
          { Name: 'instance-state-name', Values: ['running'] },
        ],
      }),
    )

    const reservations = response.Reservations || []
    let newInstanceId = null

    for (const reservation of reservations) {
      const instances = reservation.Instances || []
      for (const instance of instances) {
        if (
          instance.InstanceId &&
          instance.InstanceId !== oldInstanceId &&
          instance.InstanceId !== 'None'
        ) {
          newInstanceId = instance.InstanceId
          break
        }
      }
      if (newInstanceId) break
    }

    if (newInstanceId) {
      const isReady = await waitForSSMAgentReady(
        clients,
        newInstanceId,
        10,
        3000,
        10000,
        sleepFn,
      )
      if (isReady) {
        return newInstanceId
      }
    }

    if (attempt < maxRetries) {
      await sleepFn(retryDelayMs)
    }
  }

  return null
}

export async function waitForSSMAgentReady(
  clients,
  instanceId,
  maxRetries,
  retryDelayMs,
  stabilizationMs,
  sleepFn = defaultSleepFn,
) {
  for (let attempt = 1; attempt <= maxRetries; attempt++) {
    const response = await clients.ssm.send(
      new DescribeInstanceInformationCommand({
        Filters: [
          { Key: 'InstanceIds', Values: [instanceId] },
        ],
      }),
    )

    const instances = response.InstanceInformationList || []
    if (instances.length > 0 && instances[0].PingStatus === 'Online') {
      await sleepFn(stabilizationMs)
      return true
    }

    if (attempt < maxRetries) {
      await sleepFn(retryDelayMs)
    }
  }

  return false
}

export async function startSession(
  clients,
  instanceId,
  rdsEndpoint,
  remotePort,
  localPort,
) {
  const response = await clients.ssm.send(
    new StartSessionCommand({
      Target: instanceId,
      DocumentName: 'AWS-StartPortForwardingSessionToRemoteHost',
      Parameters: {
        host: [rdsEndpoint],
        portNumber: [String(remotePort)],
        localPortNumber: [String(localPort)],
      },
    }),
  )

  return response
}
