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
