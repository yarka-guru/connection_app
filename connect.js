#!/usr/bin/env node

// Import necessary modules
import { spawn } from 'child_process' // For spawning child processes
import inquirer from 'inquirer' // For prompting the user for input
import fs from 'fs' // For reading files
import os from 'os' // For getting the user's home directory
import path from 'path' // For working with file paths
import { envPortMapping, REGION, TABLE_NAME } from './envPortMapping.js'

// Get the path to the AWS config file
const awsConfigPath = path.join(os.homedir(), '.aws', 'config')

// Read the contents of the AWS config file
const awsConfig = fs.readFileSync(awsConfigPath, 'utf-8')

// Extract environments from AWS config file
const ENVS = awsConfig
  .split('\n')
  .filter(line => line.startsWith('[') && line.endsWith(']'))
  .map(line => line.slice(1, -1))
  .map(line => line.replace('profile ', ''))

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
    const ENV = answers.ENV // Get the selected environment from the user's answers
    console.log(`You selected: ${ENV}`)

    // Sort all environment suffixes by length, longest first
    const allEnvSuffixes = Object.keys(envPortMapping).sort((a, b) => b.length - a.length);

    // Find the first matching suffix in your envPortMapping object
    let matchedSuffix = allEnvSuffixes.find((suffix) => ENV.endsWith(suffix));

    // Get the port based on the matching suffix
    let portNumber = envPortMapping[matchedSuffix];

    // If no port number is found for the environment, default to 5432
    if (!portNumber) {
      console.error(`No port number found for environment: ${ENV}. Defaulting to 5432.`);
      portNumber = '5432';
    }

    // Set up the commands to run inside the aws-vault environment
    const awsVaultExecCommand = ['aws-vault', 'exec', ENV, '--']
    const ssmDescribeCommand = `aws ssm describe-parameters --region ${REGION} --query "Parameters[?ends_with(Name, '/rds/rds-aurora-password')].Name" --output text | head -n 1`

    // Run the commands inside aws-vault environment
    const ssmDescribeProcess = spawn('sh', ['-c', `${awsVaultExecCommand.join(' ')} ${ssmDescribeCommand}`])

    // Get the name of the parameter containing the RDS password
    ssmDescribeProcess.stdout.on('data', (data) => {
      const PARAM_NAME = data.toString().trim()

      // Get the RDS credentials
      const ssmGetCommand = `aws ssm get-parameter --region ${REGION} --name '${PARAM_NAME}' --with-decryption --query Parameter.Value --output text`
      const ssmGetProcess = spawn('sh', ['-c', `${awsVaultExecCommand.join(' ')} ${ssmGetCommand}`])

      // Parse the JSON output of the ssm get-parameter command to get the RDS credentials
      ssmGetProcess.stdout.on('data', (data) => {
        const CREDENTIALS = JSON.parse(data.toString())
        const USERNAME = CREDENTIALS.user // Get the RDS username from the credentials
        const PASSWORD = CREDENTIALS.password // Get the RDS password from the credentials

        // Display connection credentials and connection string
        console.log(`Your connection string is: psql -h localhost -p ${portNumber} -U ${USERNAME} -d ${TABLE_NAME}`)
        console.log(`Use the password: ${PASSWORD}`)

        // Get the ID of the bastion instance
        const instanceIdCommand = `aws ec2 describe-instances --region ${REGION} --filters "Name=tag:Name,Values='*bastion*'" --query "Reservations[].Instances[].[InstanceId]" --output text`
        const instanceIdProcess = spawn('sh', ['-c', `${awsVaultExecCommand.join(' ')} ${instanceIdCommand}`])

        instanceIdProcess.stdout.on('data', (data) => {
          const INSTANCE_ID = data.toString().trim()

          if (!INSTANCE_ID) {
            console.error('Failed to find the instance with tag Name=*bastion*.')
            return
          }

          // Get the endpoint of the RDS cluster
          const rdsEndpointCommand = `aws rds describe-db-clusters --region ${REGION} --query "DBClusters[?contains(DBClusterIdentifier, 'rds-aurora')].Endpoint" --output text`
          const rdsEndpointProcess = spawn('sh', ['-c', `${awsVaultExecCommand.join(' ')} ${rdsEndpointCommand}`])

          rdsEndpointProcess.stdout.on('data', (data) => {
            const RDS_ENDPOINT = data.toString().trim()

            if (!RDS_ENDPOINT) {
              console.error('Failed to find the RDS endpoint.')
              return
            }

            // Start a port forwarding session to the RDS cluster
            const portForwardingCommand = `aws ssm start-session --target ${INSTANCE_ID} --document-name AWS-StartPortForwardingSessionToRemoteHost --parameters "host=${RDS_ENDPOINT},portNumber='5432',localPortNumber='${portNumber}'" --cli-connect-timeout 0`
            const portForwardingProcess = spawn('sh', ['-c', `${awsVaultExecCommand.join(' ')} ${portForwardingCommand}`])

            portForwardingProcess.stdout.on('data', (data) => {
              console.log(data.toString().trim())
            })

            portForwardingProcess.stderr.on('data', (data) => {
              console.error(`Command execution error: ${data.toString()}`)
            })
          })

          rdsEndpointProcess.stderr.on('data', (data) => {
            console.error(`Command execution error: ${data.toString()}`)
          })
        })

        instanceIdProcess.stderr.on('data', (data) => {
          console.error(`Command execution error: ${data.toString()}`)
        })
      })

      ssmGetProcess.stderr.on('data', (data) => {
        console.error(`Command execution error: ${data.toString()}`)
      })
    })

    ssmDescribeProcess.stderr.on('data', (data) => {
      console.error(`Command execution error: ${data.toString()}`)
    })
  })
