import assert from 'node:assert/strict'
import { describe, it, mock } from 'node:test'
import {
  checkCredentialsValid,
  findBastionInstance,
  getConnectionCredentials,
  getRdsEndpoint,
  getRdsPort,
  startSession,
  terminateBastionInstance,
  waitForNewBastionInstance,
  waitForSSMAgentReady,
} from '../src/aws-operations.js'

function createMockClients(overrides = {}) {
  return {
    sts: { send: overrides.stsSend || (() => Promise.resolve({})) },
    secretsManager: {
      send: overrides.secretsManagerSend || (() => Promise.resolve({})),
    },
    ec2: { send: overrides.ec2Send || (() => Promise.resolve({})) },
    rds: { send: overrides.rdsSend || (() => Promise.resolve({})) },
    ssm: { send: overrides.ssmSend || (() => Promise.resolve({})) },
  }
}

const noopSleep = () => Promise.resolve()

describe('aws-operations', () => {
  describe('checkCredentialsValid()', () => {
    it('should return valid: true with identity when STS responds', async () => {
      const clients = createMockClients({
        stsSend: () =>
          Promise.resolve({
            Account: '123456789012',
            Arn: 'arn:aws:iam::123456789012:user/testuser',
            UserId: 'AIDAEXAMPLE',
          }),
      })

      const result = await checkCredentialsValid(clients)

      assert.equal(result.valid, true)
      assert.equal(result.identity.account, '123456789012')
      assert.equal(
        result.identity.arn,
        'arn:aws:iam::123456789012:user/testuser',
      )
      assert.equal(result.identity.userId, 'AIDAEXAMPLE')
    })

    it('should return valid: false when STS throws', async () => {
      const clients = createMockClients({
        stsSend: () => Promise.reject(new Error('ExpiredToken')),
      })

      const result = await checkCredentialsValid(clients)

      assert.equal(result.valid, false)
      assert.equal(result.error, 'ExpiredToken')
    })
  })

  describe('getConnectionCredentials()', () => {
    it('should return credentials when secret exists', async () => {
      const secretJson = JSON.stringify({
        username: 'admin',
        password: 'secret123',
      })

      let callCount = 0
      const clients = createMockClients({
        secretsManagerSend: () => {
          callCount++
          if (callCount === 1) {
            // ListSecretsCommand
            return Promise.resolve({
              SecretList: [{ Name: 'rds!cluster-abc123' }],
            })
          }
          // GetSecretValueCommand
          return Promise.resolve({
            SecretString: secretJson,
          })
        },
      })

      const result = await getConnectionCredentials(
        clients,
        'rds!cluster',
        'mydb',
      )

      assert.equal(result.username, 'admin')
      assert.equal(result.password, 'secret123')
      assert.equal(result.database, 'mydb')
      assert.equal(result.secretName, 'rds!cluster-abc123')
    })

    it('should throw when no secrets found (empty SecretList)', async () => {
      const clients = createMockClients({
        secretsManagerSend: () =>
          Promise.resolve({
            SecretList: [],
          }),
      })

      await assert.rejects(
        () => getConnectionCredentials(clients, 'rds!cluster', 'mydb'),
        { message: /No secret found matching prefix/ },
      )
    })

    it('should throw when SecretString is missing', async () => {
      let callCount = 0
      const clients = createMockClients({
        secretsManagerSend: () => {
          callCount++
          if (callCount === 1) {
            return Promise.resolve({
              SecretList: [{ Name: 'rds!cluster-abc' }],
            })
          }
          return Promise.resolve({
            SecretString: null,
          })
        },
      })

      await assert.rejects(
        () => getConnectionCredentials(clients, 'rds!cluster', 'mydb'),
        { message: /has no SecretString value/ },
      )
    })

    it('should throw when JSON is malformed', async () => {
      let callCount = 0
      const clients = createMockClients({
        secretsManagerSend: () => {
          callCount++
          if (callCount === 1) {
            return Promise.resolve({
              SecretList: [{ Name: 'rds!cluster-abc' }],
            })
          }
          return Promise.resolve({
            SecretString: '{ not valid json }',
          })
        },
      })

      await assert.rejects(
        () => getConnectionCredentials(clients, 'rds!cluster', 'mydb'),
        { message: /Failed to parse credentials/ },
      )
    })

    it('should throw when username or password missing', async () => {
      let callCount = 0
      const clients = createMockClients({
        secretsManagerSend: () => {
          callCount++
          if (callCount === 1) {
            return Promise.resolve({
              SecretList: [{ Name: 'rds!cluster-abc' }],
            })
          }
          return Promise.resolve({
            SecretString: JSON.stringify({ username: 'admin' }),
          })
        },
      })

      await assert.rejects(
        () => getConnectionCredentials(clients, 'rds!cluster', 'mydb'),
        { message: /missing required fields/ },
      )
    })
  })

  describe('findBastionInstance()', () => {
    it('should return instance ID when bastion found', async () => {
      const clients = createMockClients({
        ec2Send: () =>
          Promise.resolve({
            Reservations: [
              {
                Instances: [{ InstanceId: 'i-0abc123def456' }],
              },
            ],
          }),
      })

      const result = await findBastionInstance(clients)

      assert.equal(result, 'i-0abc123def456')
    })

    it('should throw when no reservations', async () => {
      const clients = createMockClients({
        ec2Send: () =>
          Promise.resolve({
            Reservations: [],
          }),
      })

      await assert.rejects(() => findBastionInstance(clients), {
        message: /No running bastion instance found/,
      })
    })

    it('should throw when reservations have no instances', async () => {
      const clients = createMockClients({
        ec2Send: () =>
          Promise.resolve({
            Reservations: [{ Instances: [] }],
          }),
      })

      await assert.rejects(() => findBastionInstance(clients), {
        message: /No running bastion instance found/,
      })
    })
  })

  describe('getRdsEndpoint()', () => {
    it('should return cluster endpoint for matching cluster', async () => {
      const clients = createMockClients({
        rdsSend: () =>
          Promise.resolve({
            DBClusters: [
              {
                DBClusterIdentifier: 'myapp-dev-rds-aurora',
                Status: 'available',
                Endpoint: 'myapp-dev-rds-aurora.cluster-abc.us-east-2.rds.amazonaws.com',
              },
            ],
          }),
      })

      const result = await getRdsEndpoint(clients, 'cluster', '-rds-aurora')

      assert.equal(
        result,
        'myapp-dev-rds-aurora.cluster-abc.us-east-2.rds.amazonaws.com',
      )
    })

    it('should return instance endpoint for matching instance', async () => {
      const clients = createMockClients({
        rdsSend: () =>
          Promise.resolve({
            DBInstances: [
              {
                DBInstanceIdentifier: 'covered-db-prod',
                DBInstanceStatus: 'available',
                Endpoint: {
                  Address: 'covered-db-prod.abc.us-west-1.rds.amazonaws.com',
                },
              },
            ],
          }),
      })

      const result = await getRdsEndpoint(clients, 'instance', 'covered-db')

      assert.equal(
        result,
        'covered-db-prod.abc.us-west-1.rds.amazonaws.com',
      )
    })

    it('should return null when no cluster matches', async () => {
      const clients = createMockClients({
        rdsSend: () =>
          Promise.resolve({
            DBClusters: [
              {
                DBClusterIdentifier: 'other-cluster',
                Status: 'available',
                Endpoint: 'other.endpoint.com',
              },
            ],
          }),
      })

      const result = await getRdsEndpoint(clients, 'cluster', '-rds-aurora')

      assert.equal(result, null)
    })

    it('should return null for unknown rdsType', async () => {
      const clients = createMockClients()

      const result = await getRdsEndpoint(clients, 'unknown', 'pattern')

      assert.equal(result, null)
    })
  })

  describe('getRdsPort()', () => {
    it('should return cluster port for matching cluster', async () => {
      const clients = createMockClients({
        rdsSend: () =>
          Promise.resolve({
            DBClusters: [
              {
                DBClusterIdentifier: 'myapp-dev-rds-aurora',
                Status: 'available',
                Port: 5432,
              },
            ],
          }),
      })

      const result = await getRdsPort(clients, 'cluster', '-rds-aurora', 3306)

      assert.equal(result, 5432)
    })

    it('should return instance port for matching instance', async () => {
      const clients = createMockClients({
        rdsSend: () =>
          Promise.resolve({
            DBInstances: [
              {
                DBInstanceIdentifier: 'covered-db-staging',
                DBInstanceStatus: 'available',
                Endpoint: { Port: 3306 },
              },
            ],
          }),
      })

      const result = await getRdsPort(
        clients,
        'instance',
        'covered-db',
        5432,
      )

      assert.equal(result, 3306)
    })

    it('should return fallback port when no cluster matches', async () => {
      const clients = createMockClients({
        rdsSend: () =>
          Promise.resolve({
            DBClusters: [],
          }),
      })

      const result = await getRdsPort(clients, 'cluster', '-rds-aurora', 5432)

      assert.equal(result, 5432)
    })

    it('should return fallback port for unknown rdsType', async () => {
      const clients = createMockClients()

      const result = await getRdsPort(clients, 'unknown', 'pattern', 5432)

      assert.equal(result, 5432)
    })
  })

  describe('terminateBastionInstance()', () => {
    it('should call EC2 terminate with the correct instance ID', async () => {
      const sendFn = mock.fn(() => Promise.resolve({}))
      const clients = createMockClients({ ec2Send: sendFn })

      await terminateBastionInstance(clients, 'i-0abc123')

      assert.equal(sendFn.mock.calls.length, 1)
    })
  })

  describe('waitForSSMAgentReady()', () => {
    it('should return true when agent is Online immediately', async () => {
      const clients = createMockClients({
        ssmSend: () =>
          Promise.resolve({
            InstanceInformationList: [
              { InstanceId: 'i-abc123', PingStatus: 'Online' },
            ],
          }),
      })

      const result = await waitForSSMAgentReady(
        clients,
        'i-abc123',
        5,
        1000,
        1000,
        noopSleep,
      )

      assert.equal(result, true)
    })

    it('should return true after retries', async () => {
      let callCount = 0
      const clients = createMockClients({
        ssmSend: () => {
          callCount++
          if (callCount < 3) {
            return Promise.resolve({ InstanceInformationList: [] })
          }
          return Promise.resolve({
            InstanceInformationList: [
              { InstanceId: 'i-abc123', PingStatus: 'Online' },
            ],
          })
        },
      })

      const result = await waitForSSMAgentReady(
        clients,
        'i-abc123',
        5,
        1000,
        1000,
        noopSleep,
      )

      assert.equal(result, true)
      assert.equal(callCount, 3)
    })

    it('should return false after max retries', async () => {
      const clients = createMockClients({
        ssmSend: () =>
          Promise.resolve({ InstanceInformationList: [] }),
      })

      const result = await waitForSSMAgentReady(
        clients,
        'i-abc123',
        3,
        1000,
        1000,
        noopSleep,
      )

      assert.equal(result, false)
    })
  })

  describe('waitForNewBastionInstance()', () => {
    it('should return new instance ID when new bastion comes up and SSM is ready', async () => {
      let ec2CallCount = 0
      const clients = createMockClients({
        ec2Send: () => {
          ec2CallCount++
          if (ec2CallCount <= 2) {
            // First two calls: only the old instance or no instances
            return Promise.resolve({
              Reservations: [
                { Instances: [{ InstanceId: 'i-old-bastion' }] },
              ],
            })
          }
          // Third call: new instance appears
          return Promise.resolve({
            Reservations: [
              { Instances: [{ InstanceId: 'i-new-bastion' }] },
            ],
          })
        },
        ssmSend: () =>
          Promise.resolve({
            InstanceInformationList: [
              { InstanceId: 'i-new-bastion', PingStatus: 'Online' },
            ],
          }),
      })

      const result = await waitForNewBastionInstance(
        clients,
        'i-old-bastion',
        5,
        1000,
        noopSleep,
      )

      assert.equal(result, 'i-new-bastion')
    })

    it('should return null after max retries when no new instance appears', async () => {
      const clients = createMockClients({
        ec2Send: () =>
          Promise.resolve({
            Reservations: [
              { Instances: [{ InstanceId: 'i-old-bastion' }] },
            ],
          }),
      })

      const result = await waitForNewBastionInstance(
        clients,
        'i-old-bastion',
        3,
        1000,
        noopSleep,
      )

      assert.equal(result, null)
    })
  })

  describe('startSession()', () => {
    it('should return session response with correct parameters', async () => {
      const sendFn = mock.fn(() =>
        Promise.resolve({
          SessionId: 'session-abc123',
          StreamUrl: 'wss://stream.example.com',
          TokenValue: 'token-xyz',
        }),
      )
      const clients = createMockClients({ ssmSend: sendFn })

      const result = await startSession(
        clients,
        'i-bastion123',
        'mydb.cluster-abc.us-east-2.rds.amazonaws.com',
        5432,
        5433,
      )

      assert.equal(result.SessionId, 'session-abc123')
      assert.equal(result.StreamUrl, 'wss://stream.example.com')
      assert.equal(result.TokenValue, 'token-xyz')
      assert.equal(sendFn.mock.calls.length, 1)
    })
  })
})
