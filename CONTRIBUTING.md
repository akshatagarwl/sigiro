# Contributing to Sigiro

## Git Hooks

This project uses Git 2.54+ native hooks.

### First-time Setup

```bash
git config --local include.path .githook
```

### Available Hooks

- `pre-commit`: Runs `just check`