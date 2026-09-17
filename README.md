<p align="center">
  <img
    src="https://ishan-rest.vercel.app/svg/banner/dev/GitHub_Project_Tasks_Importer"
    alt="GitHub_Project_Tasks_Importer"
    width="100%"
  />
</p>

<h1 align="center">GitHub Project Tasks Importer</h1>

<p align="center">
  Import structured task backlogs into GitHub Issues and Projects.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Python-3.10%2B-3776AB?logo=python&logoColor=white">
  <img src="https://img.shields.io/badge/GitHub_CLI-required-181717?logo=github&logoColor=white">
  <img src="https://img.shields.io/badge/GitHub_Projects-supported-181717?logo=github&logoColor=white">
  <img src="https://img.shields.io/badge/License-MIT-green">
</p>

## Requirements

- Python 3.10+
- GitHub CLI (`gh`)
- Permission to create issues in the target repository
- An existing GitHub Project

## Install GitHub CLI

### Windows

Using `winget`:

```powershell
winget install --id GitHub.cli
```

Or install manually from:

[https://cli.github.com/](https://cli.github.com/)

### macOS

Using Homebrew:

```bash
brew install gh
```

### Linux

Debian/Ubuntu:

```bash
sudo apt update
sudo apt install gh
```

Other distributions:

[https://github.com/cli/cli#installation](https://github.com/cli/cli#installation)

Verify:

```bash
gh --version
```

## Authenticate

```bash
gh auth login
```

For GitHub Project operations, also enable the `project` scope:

```bash
gh auth refresh -s project
```

Verify:

```bash
gh auth status
```

## Find Your Projects

List projects you can access:

```bash
gh project list --owner YOUR_USERNAME
```

For an organization:

```bash
gh project list --owner YOUR_ORG
```

Make sure the Project name in `config.json` exactly matches an existing Project.

Example:

```json
{
  "repo": "PersianRepo/API",
  "project": "API",
  "project_owner": "PersianRepo"
}
```

> `project_owner` is the GitHub user or organization that owns the Project, not necessarily the repository owner.

## Import Tasks

1. Edit `config.json`.
2. Validate the task file:

```bash
python import_tasks.py validate
```

3. Preview what will be created:

```bash
python import_tasks.py preview
```

4. Create/import everything:

```bash
python import_tasks.py create
```

## Rerun Safely

The importer is idempotent.

It identifies managed issues using task IDs such as:

```text
[AUTH-001]
```

Rerunning the importer does not create duplicate issues.

It will:

- Create missing labels
- Create missing issues
- Add issues to the configured Project
- Apply parent/dependency relationships
- Track imported issues in `.import-state.json`

## Troubleshooting

### Project not found

Check the Project owner and name:

```bash
gh project list --owner YOUR_USERNAME
```

Then make sure `config.json` uses the exact Project name.

### Permission errors

Re-authenticate with the Project scope:

```bash
gh auth refresh -s project
gh auth status
```

You also need permission to create issues in the target repository.

### Python command not found

Try:

```bash
python import_tasks.py validate # Use python3 in linux
```

instead of:

```bash
python import_tasks.py validate # Use python3 in linux
```
