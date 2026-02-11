import fs from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'

const CONFIG_DIR = path.join(os.homedir(), '.rds-ssm-connect')
const CONFIG_FILE = 'projects.json'

export function getConfigPath() {
  return path.join(CONFIG_DIR, CONFIG_FILE)
}

export async function loadProjectConfigs(configPath) {
  const filePath = configPath || getConfigPath()
  try {
    const data = await fs.readFile(filePath, 'utf-8')
    return JSON.parse(data)
  } catch (err) {
    if (err.code === 'ENOENT') return {}
    throw err
  }
}

export async function saveProjectConfigs(configs, configPath) {
  const filePath = configPath || getConfigPath()
  await fs.mkdir(path.dirname(filePath), { recursive: true })
  await fs.writeFile(filePath, JSON.stringify(configs, null, 2) + '\n')
}

export async function saveProjectConfig(key, config, configPath) {
  const configs = await loadProjectConfigs(configPath)
  configs[key] = config
  await saveProjectConfigs(configs, configPath)
}

export async function deleteProjectConfig(key, configPath) {
  const configs = await loadProjectConfigs(configPath)
  delete configs[key]
  await saveProjectConfigs(configs, configPath)
}

const REQUIRED_FIELDS = [
  'name',
  'region',
  'database',
  'secretPrefix',
  'rdsType',
  'rdsPattern',
  'envPortMapping',
  'defaultPort',
]

const VALID_RDS_TYPES = ['cluster', 'instance']
const REGION_PATTERN = /^[a-z]{2}(-[a-z]+-\d+)$/
const PORT_PATTERN = /^\d+$/

export function validateProjectConfig(config) {
  const errors = []

  for (const field of REQUIRED_FIELDS) {
    if (config[field] === undefined || config[field] === null || config[field] === '') {
      errors.push(`Missing required field: ${field}`)
    }
  }

  if (config.rdsType && !VALID_RDS_TYPES.includes(config.rdsType)) {
    errors.push(`rdsType must be one of: ${VALID_RDS_TYPES.join(', ')}`)
  }

  if (config.region && !REGION_PATTERN.test(config.region)) {
    errors.push(`Invalid region format: ${config.region}`)
  }

  if (config.defaultPort && !PORT_PATTERN.test(config.defaultPort)) {
    errors.push(`defaultPort must be a numeric string: ${config.defaultPort}`)
  }

  if (config.envPortMapping && typeof config.envPortMapping === 'object') {
    for (const [key, value] of Object.entries(config.envPortMapping)) {
      if (!PORT_PATTERN.test(value)) {
        errors.push(`Port for "${key}" must be a numeric string: ${value}`)
      }
    }
  }

  return { valid: errors.length === 0, errors }
}
