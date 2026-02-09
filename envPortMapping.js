// Project configurations
export const PROJECT_CONFIGS = {
  tln: {
    name: 'TLN (EMR)',
    region: 'us-east-2',
    database: 'emr',
    secretPrefix: 'rds!cluster',
    rdsType: 'cluster', // Aurora cluster
    rdsPattern: '-rds-aurora', // DBClusterIdentifier ends with this
    profileFilter: null, // Show all profiles (legacy behavior)
    envPortMapping: {
      dev: '5433',
      stage: '5434',
      'pre-prod': '5435',
      prod: '5436',
      dev2: '5437',
      stage2: '5438',
      sandbox: '5439',
      'perf-dev': '5440',
      support: '5441',
      team1: '5442',
      team2: '5443',
      team3: '5444',
      team4: '5445',
      team5: '5446',
      qa1: '5447',
      qa2: '5448',
      qa3: '5449',
      qa4: '5450',
      qa5: '5451',
      hotfix: '5452'
    },
    defaultPort: '5432'
  },
  covered: {
    name: 'Covered (Healthcare)',
    region: 'us-west-1',
    database: 'covered_db',
    secretPrefix: 'rds!db',
    rdsType: 'instance', // Single RDS instance (not Aurora)
    rdsPattern: 'covered-db', // DBInstanceIdentifier contains this
    profileFilter: 'covered', // Only show profiles starting with 'covered'
    envPortMapping: {
      'covered': '5460',
      'covered-staging': '5461'
    },
    defaultPort: '5460'
  }
}

// Legacy exports for backward compatibility
export const envPortMapping = PROJECT_CONFIGS.tln.envPortMapping
export const TABLE_NAME = PROJECT_CONFIGS.tln.database
export const REGION = PROJECT_CONFIGS.tln.region
