#!/usr/bin/env bash
set -euo pipefail

PREVIEW=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --preview)
            PREVIEW=true
            shift
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--preview]"
            exit 1
            ;;
    esac
done

if [ "$PREVIEW" = true ]; then
    if [[ "$OSTYPE" != "darwin"* ]]; then
        echo "Error: --preview is only supported on macOS"
        exit 1
    fi
fi

check_dependency() {
    local cmd=$1
    local install_msg=$2

    if ! command -v "$cmd" &> /dev/null; then
        echo "Error: '$cmd' is not installed."
        echo "$install_msg"
        exit 1
    fi
}

check_dependency "vhs" "Install vhs with: brew install vhs (macOS) or see https://github.com/charmbracelet/vhs"
check_dependency "gifsicle" "Install gifsicle with: brew install gifsicle (macOS) or apt-get install gifsicle (Linux)"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$REPO_ROOT"

# Build treeward in release mode
echo "Building treeward..."
cargo build --release

# Ensure treeward is in PATH for the demo
export PATH="$REPO_ROOT/target/release:$PATH"

# Create a temporary demo directory
DEMO_DIR=$(mktemp -d)
trap "rm -rf '$DEMO_DIR'" EXIT

cd "$DEMO_DIR"

# Create some sample files
echo "Hello, world!" > file1.txt
echo "Sample data" > file2.txt
mkdir subdir
echo "Nested file" > subdir/file3.txt

# Run VHS to generate the GIF
echo "Generating demo GIF..."
vhs "$SCRIPT_DIR/demo.tape"

# Optimize with gifsicle
echo "Optimizing GIF..."
gifsicle -O3 --colors 256 demo.gif -o demo-optimized.gif

# Move the optimized GIF to the docs directory
mv demo-optimized.gif "$SCRIPT_DIR/demo.gif"

echo "Demo GIF generated: $SCRIPT_DIR/demo.gif"

if [ "$PREVIEW" = true ]; then
    echo "Launching preview..."
    qlmanage -p "$SCRIPT_DIR/demo.gif" &> /dev/null
else
    echo "On macOS, preview with: qlmanage -p docs/demogif/demo.gif"
fi
