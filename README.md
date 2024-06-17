# AWS RDS SSM Connector

This Node.js application allows you to select an AWS environment and execute various AWS commands within that environment to connect to an RDS database securely. The environments are read from your AWS configuration file, and the application retrieves credentials and other necessary information using AWS Secrets Manager and other AWS services.

## Prerequisites

Before running this application, make sure you have the following installed:

- **Node.js**: You can download it from the [official website](https://nodejs.org/).
- **`aws-vault` tool**: You can install it following the instructions on the [official GitHub page](https://github.com/99designs/aws-vault).
- **AWS CLI**: You can install it following the instructions on the [official AWS page](https://aws.amazon.com/cli/).

Additionally, ensure that your AWS configuration file (`~/.aws/config`) is appropriately set up with the environments you want to use.

## Installation

You can install this application globally using npm:

```bash
npm install -g rds_ssm_connect
```

## Connecting to the Database

1. **Invoke the Application**:
   
   Run the following command in your terminal:

   ```bash
   rds_ssm_connect
   ```

   The application will read your AWS configuration file and prompt you to select an environment.

2. **Select an Environment**:

   The application will list the environments found in your AWS configuration file. Select the desired environment for which you want to connect to the RDS instance.

3. **Execution of AWS Commands**:

   After selecting the environment, the application will:
   
   - Extract environments from the AWS configuration file.
   - Use AWS Secrets Manager to list and retrieve the secret containing the RDS credentials.
   - Display the connection credentials and connection string.
   - Get the ID of the bastion instance.
   - Get the endpoint of the RDS cluster.
   - Provide a command to start a port forwarding session to the RDS cluster.

4. **Receive Connection Information**:

   After executing the necessary AWS commands, the application will provide the connection information, as shown below:

   ```
   Your connection string is: psql -h localhost -p <port> -U <username> -d <database>
   Use the password: <password>
   ```

5. **Port Forwarding**:

   Use the provided command to start port forwarding. This step is crucial as it sets up the local port to tunnel to the RDS cluster through the bastion host.

   For example:

   ```
   aws ssm start-session --target <instance-id> --document-name AWS-StartPortForwardingSessionToRemoteHost --parameters "host=<rds-endpoint>,portNumber='5432',localPortNumber='<port>'" --cli-connect-timeout 0
   ```

6. **Connect to Your Database**:

   Use the provided connection string and password to connect to your database via a database administration tool of your choice, such as pgAdmin, DBeaver, or the `psql` command-line interface.

   Ensure that the database administration tool is installed and configured on your local machine.

## Requirements

This application requires the following Node.js modules:

- `@aws-sdk/client-ec2`
- `@aws-sdk/client-rds`
- `@aws-sdk/client-secrets-manager`
- `inquirer`

These modules will be automatically installed when you install the application with npm.

## How It Works

1. **Reading AWS Configuration**:

   The application first reads the AWS configuration file (`~/.aws/config`) and extracts the environments configured.

2. **User Prompt**:

   The user is prompted to select one of the configured environments using `inquirer`.

3. **AWS Commands Execution**:

   Upon selecting an environment, the application performs the following operations using the AWS SDK and `aws-vault`:
   
   - **Listing Secrets**: Uses AWS Secrets Manager to list secrets and identify the one containing the RDS credentials.
   - **Retrieving Secret Value**: Fetches the secret value containing the RDS username and password.
   - **Describing Instances**: Gets the ID of a bastion instance tagged with `Name=*bastion*`.
   - **Describing RDS Clusters**: Retrieves the endpoint of the RDS cluster identified with `-rds-aurora`.
   - **Port Forwarding Command**: Outputs a command to start an AWS SSM session for port forwarding.

4. **Output**:

   The application displays the connection string and password for the RDS database along with a command to start a port forwarding session to the RDS cluster, allowing secure access from the local machine.

### Code File

Here is the `connect.js` file used in the application:

```javascript
#!/usr/bin/env node

import { EC2Client, DescribeInstancesCommand } from "@aws-sdk/client-ec2";
import { RDSClient, DescribeDBClustersCommand } from "@aws-sdk/client-rds";
import { SecretsManagerClient, GetSecretValueCommand, ListSecretsCommand } from "@aws-sdk/client-secrets-manager";
import inquirer from 'inquirer';
import fs from 'fs';
import os from 'os';
import path from 'path';
import { envPortMapping, REGION, TABLE_NAME } from './envPortMapping.js';

// Load AWS config
const awsConfigPath = path.join(os.homedir(), '.aws', 'config');
const awsConfig = fs.readFileSync(awsConfigPath, 'utf-8');
const ENVS = awsConfig
  .split('\n')
  .filter(line => line.startsWith('[') && line.endsWith(']'))
  .map(line => line.slice(1, -1))
  .map(line => line.replace('profile ', ''));

// Initialize AWS SDK clients
const ec2Client = new EC2Client({ region: REGION });
const rdsClient = new RDSClient({ region: REGION });
const secretsManagerClient = new SecretsManagerClient({ region: REGION });

// Function to list secrets
async function listSecrets() {
  const command = new ListSecretsCommand({
    Filters: [{ Key: 'name', Values: ['rds!cluster'] }]
  });
  const response = await secretsManagerClient.send(command);
  return response.SecretList.map(secret => secret.Name);
}

// Function to get secret value
async function getSecretValue(secretName) {
  const command = new GetSecretValueCommand({ SecretId: secretName });
  const response = await secretsManagerClient.send(command);
  return JSON.parse(response.SecretString);
}

// Function to get instance ID
async function getInstanceId() {
  const command = new DescribeInstancesCommand({
    Filters: [
      { Name: 'tag:Name', Values: ['*bastion*'] },
      { Name: 'instance-state-name', Values: ['running'] }
    ]
  });
  const response = await ec2Client.send(command);
  const instances = response.Reservations.flatMap(reservation => reservation.Instances.map(instance => instance.InstanceId));
  return instances[0];
}

// Function to get RDS endpoint
async function getRdsEndpoint() {
  const command = new DescribeDBClustersCommand({
    Filters: [{ Name: 'Status', Values: ['available'] }]
  });
  const response = await rdsClient.send(command);
  const clusters = response.DBClusters.filter(cluster => cluster.DBClusterIdentifier.endsWith('-rds-aurora'));
  return clusters[0].Endpoint;
}

async function main() {
  const answers = await inquirer.prompt([
    {
      type: 'list',
      name: 'ENV',
      message: 'Please select the environment:',
      choices: ENVS
    }
  ]);

  const ENV = answers.ENV;
  console.log(`You selected: ${ENV}`);

  // Sort all environment suffixes by length, longest first
  const allEnvSuffixes = Object.keys(envPortMapping).sort((a, b) => b.length - a.length);
  const matchedSuffix = allEnvSuffixes.find(suffix => ENV.endsWith(suffix));
  let portNumber = envPortMapping[matchedSuffix] || '5432';

  if (portNumber === '5432') {
    console.error(`No port number found for environment: ${ENV}. Defaulting to 5432.`);
  }

  // List secrets and get the first matching secret name
  const secretNames = await listSecrets();
  const SECRET_NAME = secretNames.find(name => name.startsWith('rds!cluster'));
  if (!SECRET_NAME) {
    console.error('No secret found with name starting with rds!cluster.');
    return;
  }

  // Get secret value
  const CREDENTIALS = await getSecretValue(SECRET_NAME);
  const USERNAME = CREDENTIALS.username;
  const PASSWORD = CREDENTIALS.password;

  console.log(`Your connection string is: psql -h localhost -p ${portNumber} -U ${USERNAME} -d ${TABLE_NAME}`);
  console.log(`Use the password: ${PASSWORD}`);

  // Get instance ID
  const INSTANCE_ID = await getInstanceId();
  if (!INSTANCE_ID) {
    console.error('Failed to find a running instance with tag Name=*bastion*.');
    return;
  }

  // Get RDS endpoint
  const RDS_ENDPOINT = await getRdsEndpoint();
  if (!RDS_ENDPOINT) {
    console.error('Failed to find the RDS endpoint.');
    return;
  }

  // Start a port forwarding session (this requires AWS CLI tool as the SDK doesn't support starting sessions)
  const portForwardingCommand = `aws ssm start-session --target ${INSTANCE_ID} --document-name AWS-StartPortForwardingSessionToRemoteHost --parameters "host=${RDS_ENDPOINT},portNumber='5432',localPortNumber='${portNumber}'" --cli-connect-timeout 0`;
  console.log(`Run this command to start port forwarding: ${portForwardingCommand}`);
}

main().catch(err => {
  console.error(err);
});
```

This documentation provides detailed instructions on installation, usage, prerequisites, and how the system works, making it easy for users to understand and use your application effectively.