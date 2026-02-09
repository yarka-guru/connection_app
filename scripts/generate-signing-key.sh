#!/bin/bash

# Generate Tauri signing keys for auto-updates
# Run this once and save the keys securely

echo "Generating Tauri signing keys..."
echo ""

# Generate the key pair
npx @tauri-apps/cli signer generate -w ~/.tauri/rds-ssm-connect.key

echo ""
echo "============================================"
echo "IMPORTANT: Save these values as GitHub secrets:"
echo "============================================"
echo ""
echo "1. TAURI_SIGNING_PRIVATE_KEY"
echo "   Copy the ENTIRE contents of ~/.tauri/rds-ssm-connect.key"
echo ""
echo "2. TAURI_SIGNING_PRIVATE_KEY_PASSWORD"
echo "   The password you entered (or empty if none)"
echo ""
echo "3. Update src-tauri/tauri.conf.json"
echo "   Set 'plugins.updater.pubkey' to the PUBLIC KEY shown above"
echo ""
echo "Go to: https://github.com/YOUR_USERNAME/connection_app/settings/secrets/actions"
echo ""
