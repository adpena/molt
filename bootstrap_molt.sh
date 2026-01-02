#!/bin/bash
set -e

echo "🚀 Bootstrapping Molt development environment..."

# Check for uv
if ! command -v uv &> /dev/null; then
    echo "❌ uv not found. Please install it: https://github.com/astral-sh/uv"
    exit 1
fi

# Setup Python environment
echo "🐍 Setting up Python environment..."
uv venv
source .venv/bin/activate
uv sync

# Check for Rust
if ! command -v cargo &> /dev/null; then
    echo "❌ Rust/Cargo not found. Please install it: https://rustup.rs/"
    exit 1
fi

# Build runtime
echo "🦀 Building Molt runtime..."
cargo build --release

echo "✅ Environment ready! Try running 'molt --help'"