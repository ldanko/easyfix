#!/bin/bash
# Generate easyfix dictionary XML from FIX Repository 2010 Edition
#
# This script converts the FIX Repository 2010 Edition XML files into
# XML dictionaries compatible with easyfix-dictionary.
#
# Usage:
#   ./generate_fix_xml.sh [--docs] [REPO_DIR]
#
# Arguments:
#   --docs   - Emit doc attributes with documentation from the repository
#   REPO_DIR - Path to FIX Repository 2010 Edition (default: ../fix_repository_2010_edition_20200402)
#
# Output:
#   ../easyfix-messages/xml/FIXT11.xml
#   ../easyfix-messages/xml/FIX50SP2.xml

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

DOCS_FLAG=""
REPO_DIR_ARG=""
for arg in "$@"; do
    case "$arg" in
        --docs) DOCS_FLAG="--docs" ;;
        *) REPO_DIR_ARG="$arg" ;;
    esac
done

REPO_DIR="${REPO_DIR_ARG:-${SCRIPT_DIR}/../fix_repository_2010_edition_20200402}"
OUTPUT_DIR="${SCRIPT_DIR}/../easyfix-messages/xml"

if [ ! -d "${REPO_DIR}" ]; then
    echo "Error: FIX Repository directory not found: ${REPO_DIR}"
    echo "Download from: https://www.fixtrading.org/standards/fix-repository/"
    exit 1
fi

echo "Generating FIXT11.xml..."
python3 "${SCRIPT_DIR}/repo_to_easyfix.py" \
    --repo-dir "${REPO_DIR}" \
    --version FIXT.1.1 \
    --output "${OUTPUT_DIR}/FIXT11.xml" \
    ${DOCS_FLAG}

echo ""
echo "Generating FIX50SP2.xml..."
python3 "${SCRIPT_DIR}/repo_to_easyfix.py" \
    --repo-dir "${REPO_DIR}" \
    --version FIX.5.0SP2 \
    --output "${OUTPUT_DIR}/FIX50SP2.xml" \
    ${DOCS_FLAG}

echo ""
echo "XML files generated successfully."
echo ""
echo "Validating with easyfix-dictionary tests..."
cd "${SCRIPT_DIR}/.."
cargo test -p easyfix-dictionary

echo ""
echo "Done. Generated files:"
echo "  ${OUTPUT_DIR}/FIXT11.xml"
echo "  ${OUTPUT_DIR}/FIX50SP2.xml"
