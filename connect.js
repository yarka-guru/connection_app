#!/usr/bin/env node

// Import necessary modules
import { spawn } from 'child_process'; // For spawning child processes
import inquirer from 'inquirer'; // For prompting the user for input
import fs from 'fs'; // For reading files
import os from 'os'; // For getting the user's home directory
import path from 'path'; // For working with file paths
import { envPortMapping, REGION, TABLE_NAME } from './envPortMapping.js';

// Get the path to the AWS config file
const awsConfigPath = path.join(os.homedir(), '.aws', 'config');

// Read the contents of the AWS config file
const awsConfig = fs.readFileSync(awsConfigPath, 'utf-8');

// Extract environments from the AWS config file
const ENVS = awsConfig
  .split('\n')
  .filter(line => line.startsWith('[') && line.endsWith(']'))
  .map(line => line.slice(1, -1))
  .map(line => line.replace('profile ', ''));

// Prompt the user to select an environment
inquirer
  .prompt([
    {
      type: 'list',
      name: 'ENV',
      message: 'Please select the environment:',
      choices: ENVS
    }
  ])
  .then((answers) => {
    const ENV = answers.ENV; // Get the selected environment from the user's answers
    console.log(`You selected: ${ENV}`);

    // Sort all environment suffixes by length, longest first
    const allEnvSuffixes = Object.keys(envPortMapping).sort((a, b) => b.length - a.length);

    // Find the first matching suffix in your envPortMapping object
    const matchedSuffix = allEnvSuffixes.find(suffix => ENV.endsWith(suffix));

    // If no port number is found for the environment, default to 5432
    let portNumber = envPortMapping[matchedSuffix];

    if (!portNumber) {
      console.error(`No port number found for environment: ${ENV}. Defaulting to 5432.`);
      portNumber = '5432';
    }

    // Set up the commands to run inside the aws-vault environment
    const awsVaultExecCommand = ['aws-vault', 'exec', ENV, '--'];
    const secretsDescribeCommand = `aws secretsmanager list-secrets --region ${REGION} --query 'SecretList[?starts_with(Name, \`rds!cluster\`)].Name' --output text | head -n 1`;

    // Run the commands inside aws-vault environment
    const secretsDescribeProcess = spawn('sh', ['-c', `${awsVaultExecCommand.join(' ')} ${secretsDescribeCommand}`]);

    // Get the name of the secret containing the RDS credentials
    secretsDescribeProcess.stdout.on('data', (data) => {
      const SECRET_NAME = data.toString().trim();

      if (!SECRET_NAME) {
        console.error('No secret found with name starting with rds!cluster.');
        return;
      }

      // Get the RDS credentials from Secrets Manager
      const secretsGetCommand = `aws secretsmanager get-secret-value --region ${REGION} --secret-id '${SECRET_NAME}' --query SecretString --output text`;
      const secretsGetProcess = spawn('sh', ['-c', `${awsVaultExecCommand.join(' ')} ${secretsGetCommand}`]);

      // Parse the JSON output of the secretsmanager get-secret-value command to get the RDS credentials
      secretsGetProcess.stdout.on('data', (data) => {
        const CREDENTIALS = JSON.parse(data.toString());
        const USERNAME = CREDENTIALS.username; // Get the RDS username from the credentials
        const PASSWORD = CREDENTIALS.password; // Get the RDS password from the credentials

        // Display connection credentials and connection string
        console.log(`Your connection string is: psql -h localhost -p ${portNumber} -U ${USERNAME} -d ${TABLE_NAME}`);
        console.log(`Use the password: ${PASSWORD}`);

        // Get the ID of the bastion instance in running state
        const instanceIdCommand = `aws ec2 describe-instances --region ${REGION} --filters "Name=tag:Name,Values='*bastion*'" "Name=instance-state-name,Values=running" --query "Reservations[].Instances[].[InstanceId]" --output text`;
        const instanceIdProcess = spawn('sh', ['-c', `${awsVaultExecCommand.join(' ')} ${instanceIdCommand}`]);

        instanceIdProcess.stdout.on('data', (data) => {
          const INSTANCE_ID = data.toString().trim();

          if (!INSTANCE_ID) {
            console.error('Failed to find a running instance with tag Name=*bastion*.');
            return;
          }

          // Get the endpoint of the RDS cluster
          const rdsEndpointCommand = `aws rds describe-db-clusters --region ${REGION} --query "DBClusters[?Status=='available' && ends_with(DBClusterIdentifier, '-rds-aurora')].Endpoint" --output text`;
          const rdsEndpointProcess = spawn('sh', ['-c', `${awsVaultExecCommand.join(' ')} ${rdsEndpointCommand}`]);

          rdsEndpointProcess.stdout.on('data', (data) => {
            const RDS_ENDPOINT = data.toString().trim();

            if (!RDS_ENDPOINT) {
              console.error('Failed to find the RDS endpoint.');
              return;
            }

            // Start a port forwarding session to the RDS cluster
            const portForwardingCommand = `aws ssm start-session --target ${INSTANCE_ID} --document-name AWS-StartPortForwardingSessionToRemoteHost --parameters "host=${RDS_ENDPOINT},portNumber='5432',localPortNumber='${portNumber}'" --cli-connect-timeout 0`;
            const portForwardingProcess = spawn('sh', ['-c', `${awsVaultExecCommand.join(' ')} ${portForwardingCommand}`]);

            portForwardingProcess.stdout.on('data', (data) => {
              console.log(data.toString().trim());
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

      secretsGetProcess.stderr.on('data', (data) => {
        console.error(`Command execution error: ${data.toString()}`);
      });
    });

    secretsDescribeProcess.stderr.on('data', (data) => {
      console.error(`Command execution error: ${data.toString()}`);
    });
  });