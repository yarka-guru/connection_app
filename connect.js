// Import necessary modules
import { spawn } from 'child_process'; // For spawning child processes
import inquirer from 'inquirer'; // For prompting the user for input
import fs from 'fs'; // For reading files
import os from 'os'; // For getting the user's home directory
import path from 'path'; // For working with file paths

// Read AWS config file
const awsConfigPath = path.join(os.homedir(), '.aws', 'config'); // Get the path to the AWS config file
const awsConfig = fs.readFileSync(awsConfigPath, 'utf-8'); // Read the contents of the AWS config file

// Extract environments from AWS config file
const ENVS = awsConfig
  .split('\n') // Split the contents of the file into an array of lines
  .filter(line => line.startsWith('[') && line.endsWith(']')) // Filter out lines that don't start with '[' and end with ']'
  .map(line => line.slice(1, -1)); // Remove the '[' and ']' characters from the start and end of each line

// Ask user to select the environment
inquirer
  .prompt([
    {
      type: 'list',
      name: 'ENV',
      message: 'Please select the environment:', // Prompt the user to select an environment
      choices: ENVS, // Use the list of environments extracted from the AWS config file as the choices
    },
  ])
  .then((answers) => {
    const ENV = answers.ENV; // Get the selected environment from the user's answers
    console.log(`You selected: ${ENV}`);

    // Set up the commands to run inside the aws-vault environment
    const awsVaultExecCommand = ['aws-vault', 'exec', ENV, '--no-session', '--'];
    const ssmDescribeCommand = 'aws ssm describe-parameters --region us-east-2 --query \'Parameters[?ends_with(Name, `/rds/rds-aurora-password`)].Name\' --output text | head -n 1';

    // Run the commands inside aws-vault environment
    const ssmDescribeProcess = spawn('sh', ['-c', `${awsVaultExecCommand.join(' ')} ${ssmDescribeCommand}`]);

    ssmDescribeProcess.stdout.on('data', (data) => {
      const PARAM_NAME = data.toString().trim(); // Get the name of the parameter containing the RDS password

      const ssmGetCommand = `aws ssm get-parameter --region us-east-2 --name '${PARAM_NAME}' --with-decryption --query Parameter.Value --output text`;

      const ssmGetProcess = spawn('sh', ['-c', `${awsVaultExecCommand.join(' ')} ${ssmGetCommand}`]);

      ssmGetProcess.stdout.on('data', (data) => {
        const CREDENTIALS = JSON.parse(data.toString()); // Parse the JSON output of the ssm get-parameter command to get the RDS credentials
        const USERNAME = CREDENTIALS.user; // Get the RDS username from the credentials
        const PASSWORD = CREDENTIALS.password; // Get the RDS password from the credentials

        // Display connection credentials and connection string
        console.log(`Your connection string is: psql -h localhost -p 5432 -U ${USERNAME} -d emr`);
        console.log(`Use the password: ${PASSWORD}`);

        const instanceIdCommand = `aws ec2 describe-instances --region us-east-2 --filters "Name=tag:Name,Values='*bastion*'" --query "Reservations[].Instances[].[InstanceId]" --output text`;

        const instanceIdProcess = spawn('sh', ['-c', `${awsVaultExecCommand.join(' ')} ${instanceIdCommand}`]);

        instanceIdProcess.stdout.on('data', (data) => {
          const INSTANCE_ID = data.toString().trim(); // Get the ID of the bastion instance

          if (!INSTANCE_ID) {
            console.error('Failed to find the instance with tag Name=*bastion*.'); // If the bastion instance is not found, log an error and return
            return;
          }

          const rdsEndpointCommand = `aws rds describe-db-clusters --region us-east-2 --query "DBClusters[?contains(DBClusterIdentifier, 'rds-aurora')].Endpoint" --output text`;

          const rdsEndpointProcess = spawn('sh', ['-c', `${awsVaultExecCommand.join(' ')} ${rdsEndpointCommand}`]);

          rdsEndpointProcess.stdout.on('data', (data) => {
            const RDS_ENDPOINT = data.toString().trim(); // Get the endpoint of the RDS cluster

            if (!RDS_ENDPOINT) {
              console.error('Failed to find the RDS endpoint.'); // If the RDS endpoint is not found, log an error and return
              return;
            }

            const portForwardingCommand = `aws ssm start-session --target ${INSTANCE_ID} --document-name AWS-StartPortForwardingSessionToRemoteHost --parameters "host=${RDS_ENDPOINT},portNumber='5432',localPortNumber='5432'" --cli-connect-timeout 0`;

            const portForwardingProcess = spawn('sh', ['-c', `${awsVaultExecCommand.join(' ')} ${portForwardingCommand}`]);

            portForwardingProcess.stdout.on('data', (data) => {
              console.log(data.toString().trim()); // Log the output of the port forwarding process
              // Handle the successful start of the port forwarding session
            });

            portForwardingProcess.stderr.on('data', (data) => {
              console.error(`Command execution error: ${data.toString()}`); // If there is an error with the port forwarding process, log the error
            });
          });

          rdsEndpointProcess.stderr.on('data', (data) => {
            console.error(`Command execution error: ${data.toString()}`); // If there is an error with the RDS endpoint process, log the error
          });
        });

        instanceIdProcess.stderr.on('data', (data) => {
          console.error(`Command execution error: ${data.toString()}`); // If there is an error with the bastion instance ID process, log the error
        });
      });

      ssmGetProcess.stderr.on('data', (data) => {
        console.error(`Command execution error: ${data.toString()}`); // If there is an error with the ssm get-parameter process, log the error
      });
    });

    ssmDescribeProcess.stderr.on('data', (data) => {
      console.error(`Command execution error: ${data.toString()}`); // If there is an error with the ssm describe-parameters process, log the error
    });
  });
