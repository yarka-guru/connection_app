# AWS Environment Selector and Command Executor

This Node.js application allows you to select an AWS environment and execute various AWS commands within that environment. The environments are read from your AWS configuration file, and the application uses the `aws-vault` tool to manage your AWS credentials securely.

## Prerequisites

Before running this application, make sure you have the following installed:

- Node.js: You can download it from the [official website](https://nodejs.org/).
- `aws-vault` tool: You can install it following the instructions on the [official GitHub page](https://github.com/99designs/aws-vault).
- AWS CLI: You can install it following the instructions on the [official AWS page](https://aws.amazon.com/cli/).

Also, ensure that your AWS configuration file (`~/.aws/config`) is appropriately set up with the environments you want to use.

## Installation

You can install this application globally using npm:

```bash
npm install -g rds_oit_connect
```

## Usage

1. Run the application. It will read your AWS configuration file and prompt you to select an environment.

```bash
rds_oit_connect
```

2. Select the environment you want to use. The application will then execute a series of AWS commands within that environment.

## Requirements

This application requires the following Node.js modules:

- `child_process`: For spawning child processes to execute AWS commands.
- `inquirer`: For prompting the user to select an environment.
- `fs`: For reading the AWS configuration file.
- `os`: For getting the user's home directory.
- `path`: For working with file paths.

These modules will be installed automatically when you install the application with npm.

## How It Works

The application first reads the AWS configuration file and extracts the environments from it. It then prompts the user to select an environment.

After the user selects an environment, the application executes a series of AWS commands within that environment using `aws-vault`. These commands include:

- Describing SSM parameters to get the parameter's name containing the RDS password.
- Getting the value of the RDS password parameter.
- Describing EC2 instances to get the ID of the bastion instance.
- Describing RDS DB clusters to get the endpoint of the RDS cluster.
- Starting an AWS SSM session to forward a local port to the RDS cluster.

The application logs the output of each command and any errors that occur.