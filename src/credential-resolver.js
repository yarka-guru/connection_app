import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import readline from 'node:readline';
import {
  fromEnv,
  fromNodeProviderChain,
} from '@aws-sdk/credential-providers';

export function isRunningUnderAwsVault() {
  return !!process.env.AWS_VAULT;
}

export async function parseAwsConfig() {
  const configPath = path.join(os.homedir(), '.aws', 'config');

  let content;
  try {
    content = await fs.readFile(configPath, { encoding: 'utf-8' });
  } catch {
    return {};
  }

  const profiles = {};
  let currentProfile = null;

  for (const rawLine of content.split(/\r?\n/)) {
    const line = rawLine.trim();

    if (!line || line.startsWith('#') || line.startsWith(';')) {
      continue;
    }

    if (line.startsWith('[') && line.endsWith(']')) {
      let sectionName = line.slice(1, -1).trim();
      if (sectionName.startsWith('profile ')) {
        sectionName = sectionName.slice('profile '.length).trim();
      }
      currentProfile = sectionName;
      if (!profiles[currentProfile]) {
        profiles[currentProfile] = {};
      }
      continue;
    }

    if (currentProfile !== null) {
      const eqIndex = line.indexOf('=');
      if (eqIndex !== -1) {
        const key = line.slice(0, eqIndex).trim();
        const value = line.slice(eqIndex + 1).trim();
        profiles[currentProfile][key] = value;
      }
    }
  }

  return profiles;
}

export async function detectAuthType(profile) {
  const profiles = await parseAwsConfig();
  const config = profiles[profile] || {};

  if (config.sso_start_url || config.sso_session) {
    return {
      type: 'sso',
      details: {
        sso_start_url: config.sso_start_url || null,
        sso_session: config.sso_session || null,
        sso_account_id: config.sso_account_id || null,
        sso_role_name: config.sso_role_name || null,
      },
    };
  }

  if (config.role_arn && config.mfa_serial) {
    return {
      type: 'assume-role-mfa',
      details: {
        role_arn: config.role_arn,
        mfa_serial: config.mfa_serial,
        source_profile: config.source_profile || null,
      },
    };
  }

  if (config.role_arn) {
    return {
      type: 'assume-role',
      details: {
        role_arn: config.role_arn,
        source_profile: config.source_profile || null,
      },
    };
  }

  if (config.credential_process) {
    return {
      type: 'process',
      details: {
        credential_process: config.credential_process,
      },
    };
  }

  if (config.aws_access_key_id) {
    return {
      type: 'static',
      details: {},
    };
  }

  return {
    type: 'unknown',
    details: {},
  };
}

function defaultMfaPrompt(serialArn) {
  return new Promise((resolve, reject) => {
    const rl = readline.createInterface({
      input: process.stdin,
      output: process.stderr,
    });
    rl.question(`MFA code for ${serialArn}: `, (answer) => {
      rl.close();
      const code = answer?.trim();
      if (code) {
        resolve(code);
      } else {
        reject(new Error('No MFA code provided'));
      }
    });
  });
}

/**
 * Extract SSO config for a profile. Handles both legacy SSO (keys on profile)
 * and new-style sso-session sections. Returns null for non-SSO profiles.
 */
export async function getSsoConfig(profile) {
  const profiles = await parseAwsConfig()
  const config = profiles[profile] || {}

  let startUrl = config.sso_start_url
  let region = config.sso_region
  const accountId = config.sso_account_id || null
  const roleName = config.sso_role_name || null

  // New-style: profile references an [sso-session <name>] section
  if (!startUrl && config.sso_session) {
    const sessionConfig = profiles[`sso-session ${config.sso_session}`] || {}
    startUrl = sessionConfig.sso_start_url
    region = region || sessionConfig.sso_region
  }

  if (!startUrl || !region) return null

  return { startUrl, region, accountId, roleName }
}

export function resolveCredentials(profile, options = {}) {
  const mfaCodeProvider = options.mfaPrompt || defaultMfaPrompt;

  if (isRunningUnderAwsVault()) {
    return fromEnv();
  }

  if (process.env.AWS_PROFILE && !profile) {
    return fromNodeProviderChain({ mfaCodeProvider });
  }

  return fromNodeProviderChain({
    profile: profile || undefined,
    mfaCodeProvider,
  });
}
