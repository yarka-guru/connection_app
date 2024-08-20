#!/usr/bin/env node

import inquirer from 'inquirer'
import fs from 'fs/promises'
import os from 'os'
import path from 'path'
import { exec } from 'child_process'
import { promisify } from 'util'
import { envPortMapping, REGION, TABLE_NAME } from './envPortMapping.js'

const execAsync = promisify(exec)

async function readAwsConfig () {
  const awsConfigPath = path.join(os.homedir(), '.aws', 'config')
  try {
    const awsConfig = await fs.readFile(awsConfigPath, 'utf-8')
    return awsConfig
      .split('\n')
      .filter(line => line.startsWith('[') && line.endsWith(']'))
      .map(line => line.slice(1, -1))
      .map(line => line.replace('profile ', ''))
  } catch (error) {
    console.error('Error reading AWS config file:', error)
    return []
  }
}

async function runCommand (command) {
  try {
    const { stdout } = await execAsync(command)
    return stdout.trim()
  } catch (error) {
    console.error(`Error executing command: ${command}`)
    console.error(error)
    return null
  }
}

async function main () {
  try {
    const ENVS = await readAwsConfig()

    if (ENVS.length === 0) {
      console.error('No environments found in AWS config file.')
      return
    }

    const answers = await inquirer.prompt([
      {
        type: 'list',
        name: 'ENV',
        message: 'Please select the environment:',
        choices: ENVS
      }
    ])

    const ENV = answers.ENV
    const allEnvSuffixes = Object.keys(envPortMapping).sort((a, b) => b.length - a.length)
    const matchedSuffix = allEnvSuffixes.find(suffix => ENV.endsWith(suffix))
    const portNumber = envPortMapping[matchedSuffix] || '5432'

    const secretsListCommand = `aws-vault exec ${ENV} -- aws secretsmanager list-secrets --region ${REGION} --query "SecretList[?starts_with(Name, 'rds!cluster')].Name | [0]" --output text`
    const SECRET_NAME = await runCommand(secretsListCommand)

    if (!SECRET_NAME) {
      console.error('No secret found with name starting with rds!cluster.')
      return
    }

    const secretsGetCommand = `aws-vault exec ${ENV} -- aws secretsmanager get-secret-value --region ${REGION} --secret-id "${SECRET_NAME}" --query SecretString --output text`
    const secretString = await runCommand(secretsGetCommand)
    const CREDENTIALS = JSON.parse(secretString)
    const USERNAME = CREDENTIALS.username
    const PASSWORD = CREDENTIALS.password

    console.log(`Your connection string is: psql -h localhost -p ${portNumber} -U ${USERNAME} -d ${TABLE_NAME}`)
    console.log(`Use the password: ${PASSWORD}`)

    const instanceIdCommand = `aws-vault exec ${ENV} -- aws ec2 describe-instances --region ${REGION} --filters "Name=tag:Name,Values='*bastion*'" "Name=instance-state-name,Values=running" --query "Reservations[].Instances[].[InstanceId] | [0][0]" --output text`
    const INSTANCE_ID = await runCommand(instanceIdCommand)

    if (!INSTANCE_ID) {
      console.error('Failed to find a running instance with tag Name=*bastion*.')
      return
    }

    const rdsEndpointCommand = `aws-vault exec ${ENV} -- aws rds describe-db-clusters --region ${REGION} --query "DBClusters[?Status=='available' && ends_with(DBClusterIdentifier, '-rds-aurora')].Endpoint | [0]" --output text`
    const RDS_ENDPOINT = await runCommand(rdsEndpointCommand)

    if (!RDS_ENDPOINT) {
      console.error('Failed to find the RDS endpoint.')
      return
    }

    const portForwardingCommand = `aws-vault exec ${ENV} -- aws ssm start-session --target ${INSTANCE_ID} --document-name AWS-StartPortForwardingSessionToRemoteHost --parameters "host=${RDS_ENDPOINT},portNumber='5432',localPortNumber='${portNumber}'" --cli-connect-timeout 0`

    console.log('Starting port forwarding session...')
    const child = exec(portForwardingCommand)

    child.stdout.on('data', (data) => {
      console.log(data.toString())
    })

    child.stderr.on('data', (data) => {
      console.error(data.toString())
    })

    child.on('close', (code) => {
      console.log(`Port forwarding session ended with code ${code}`)
    })

    console.log('Port forwarding session established. You can now connect to the database using the provided connection string.')
    console.log('Press Ctrl+C to end the session.')
  } catch (error) {
    console.error(`Error: ${error.message}`)
    throw error // Re-throw the error to be caught by the outer catch block
  }
}

main().catch(error => {
  console.error('Unhandled error in main function:', error)
  console.error('Exiting due to unhandled error')
  setImmediate(() => {
    throw new Error('Forcing exit due to unhandled error')
  })
})
