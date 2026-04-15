#!/usr/bin/env bash
set -euo pipefail

# Regenerate the pre-generated test messages for easyfix-messages integration tests.
# Run from the workspace root after making changes to the code generator.

cargo run -p easyfix-messages -- \
    --fixt-xml easyfix-test-messages/xml/FIXT11-test.xml \
    --fix-xml easyfix-test-messages/xml/FIX50SP2-test.xml \
    --output easyfix-test-messages/src/lib.rs

echo "Regenerated easyfix-test-messages/src/lib.rs"
