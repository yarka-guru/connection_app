import { spawn } from 'child_process';
import inquirer from 'inquirer';
import fs from 'fs';
import os from 'os';
import path from 'path';

// Read AWS config file
const awsConfigPath = path.join(os.homedir(), '.aws', 'config');
const awsConfig = fs.readFileSync(awsConfigPath, 'utf-8');

// Extract environments from AWS config file
const ENVS = awsConfig
  .split('\n')
  .filter(line => line.startsWith('[') && line.endsWith(']'))
  .map(line => line.slice(1, -1));

// Ask user to select the environment
inquirer
  .prompt([
    {
      type: 'list',
      name: 'ENV',
      message: 'Please select the environment:',
      choices: ENVS,
    },
  ])
  .then((answers) => {
    const ENV = answers.ENV;
    console.log(`You selected: ${ENV}`);

    const awsVaultExecCommand = ['aws-vault', 'exec', ENV, '--no-session', '--'];
    const ssmDescribeCommand = 'aws ssm describe-parameters --region us-east-2 --query \'Parameters[?ends_with(Name, `/rds/rds-aurora-password`)].Name\' --output text | head -n 1';

    // Run the commands inside aws-vault environment
    const ssmDescribeProcess = spawn('sh', ['-c', `${awsVaultExecCommand.join(' ')} ${ssmDescribeCommand}`]);

    ssmDescribeProcess.stdout.on('data', (data) => {
      const PARAM_NAME = data.toString().trim();

      const ssmGetCommand = `aws ssm get-parameter --region us-east-2 --name '${PARAM_NAME}' --with-decryption --query Parameter.Value --output text`;

      const ssmGetProcess = spawn('sh', ['-c', `${awsVaultExecCommand.join(' ')} ${ssmGetCommand}`]);

      ssmGetProcess.stdout.on('data', (data) => {
        const CREDENTIALS = JSON.parse(data.toString());
        const USERNAME = CREDENTIALS.user;
        const PASSWORD = CREDENTIALS.password;

        // Display connection credentials and connection string
        console.log(`Your connection string is: psql -h localhost -p 5432 -U ${USERNAME} -d emr`);
        console.log(`Use the password: ${PASSWORD}`);

        const instanceIdCommand = `aws ec2 describe-instances --region us-east-2 --filters "Name=tag:Name,Values='*bastion*'" --query "Reservations[].Instances[].[InstanceId]" --output text`;

        const instanceIdProcess = spawn('sh', ['-c', `${awsVaultExecCommand.join(' ')} ${instanceIdCommand}`]);

        instanceIdProcess.stdout.on('data', (data) => {
          const INSTANCE_ID = data.toString().trim();

          if (!INSTANCE_ID) {
            console.error('Failed to find the instance with tag Name=*bastion*.');
            return;
          }

          const rdsEndpointCommand = `aws rds describe-db-clusters --region us-east-2 --query "DBClusters[?contains(DBClusterIdentifier, 'rds-aurora')].Endpoint" --output text`;

          const rdsEndpointProcess = spawn('sh', ['-c', `${awsVaultExecCommand.join(' ')} ${rdsEndpointCommand}`]);

          rdsEndpointProcess.stdout.on('data', (data) => {
            const RDS_ENDPOINT = data.toString().trim();

            if (!RDS_ENDPOINT) {
              console.error('Failed to find the RDS endpoint.');
              return;
            }

            const portForwardingCommand = `aws ssm start-session --target ${INSTANCE_ID} --document-name AWS-StartPortForwardingSessionToRemoteHost --parameters "host=${RDS_ENDPOINT},portNumber='5432',localPortNumber='5432'" --cli-connect-timeout 0`;

            const portForwardingProcess = spawn('sh', ['-c', `${awsVaultExecCommand.join(' ')} ${portForwardingCommand}`]);

            portForwardingProcess.stdout.on('data', (data) => {
              console.log(data.toString().trim());
              // Handle the successful start of the port forwarding session
            });

            portForwardingProcess.stderr.on('data', (data) => {
              console.error(`Command execution error: ${data.toString()}`);
            });
          });

          rdsEndpointProcess.stderr.on('data', (data) => {
            console.error(`Command execution error: ${data.toString()}`);
          });
        });

        instanceIdProcess.stderr.on('data', (data) => {
          console.error(`Command execution error: ${data.toString()}`);
        });
      });

      ssmGetProcess.stderr.on('data', (data) => {
        console.error(`Command execution error: ${data.toString()}`);
      });
    });

    ssmDescribeProcess.stderr.on('data', (data) => {
      console.error(`Command execution error: ${data.toString()}`);
    });
  });
