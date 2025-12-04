# 📌👊 pin-and-bump

Pin GitHub Actions to commit SHAs, and optionally update to latest versions.

## Installation

```bash
cargo install pin-and-bump
```

## Usage

```bash
# Pin current versions
pin-and-bump

# Update to latest versions and pin
pin-and-bump --update

# Specify repository path
pin-and-bump -p /path/to/repo
```

## Example

**Before:**
```yaml
- uses: actions/checkout@v4
- uses: actions/setup-go@v5
```

**After (pin):**
```yaml
- uses: actions/checkout@8ade135a41bc03ea155e62e844d188df1ea18608 # v4
- uses: actions/setup-go@0a12ed9d6a9990640e88f7f159f6c4bc9925b9b2 # v5
```

**After (pin + update with `--update`):**
```yaml
- uses: actions/checkout@1234567890abcdef1234567890abcdef12345678 # v5
- uses: actions/setup-go@abcdef1234567890abcdef1234567890abcdef12 # v6
```

## Features

- Resolves tags to commit SHAs using GitHub API
- Preserves formatting, indentation, and comments
- `--update` flag fetches latest releases and updates versions

## TODO

- cut a release; first release can't use trusted publishing :sob:
- when `--update` is set, also update already-pinned deps
