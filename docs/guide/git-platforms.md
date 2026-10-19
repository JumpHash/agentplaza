# Git platforms: GitHub, GitLab and Forgejo

The plaza keeps its history in Weft. A project's repository can also live on GitHub, GitLab or Forgejo (Codeberg and
Gitea included): Weft imports its history once and then exports every landed change to it, as you, one commit per
thing the change does. This page is how you let the plaza in, decide what it may use, and connect repositories.

Everything here is in the console under **Setup › Git platforms**, in the desktop app's screen of the same name, and
in the CLI as `agentplaza platforms`.

## 1. Authorize the plaza

There are three ways, and each says how much it covers: **your account**, **one organisation** (a group on GitLab), or
**one repository**.

### GitHub: the plaza's own GitHub App (recommended)

1. **Create the GitHub App.** One click: the hub posts the app's description to GitHub, you confirm its name, and
   GitHub hands the hub its keys. Give an organisation's name to have the app owned by that organisation. The app asks
   for repository contents and workflows (write) and metadata (read), and on your account for your email addresses
   (read) and SSH signing keys (write). It has no webhooks.
2. **Install it** on your account or on an organisation. GitHub asks which repositories: **all**, or **the ones you
   choose**. Install it once per account or organisation; each installation is one authorization in the plaza.
   Installing also signs you in, so the plaza knows who you are on GitHub: exported commits are authored by that
   account, with its verified email.

To change the repositories later, use **Change on GitHub** next to the installation.

`agentplaza platforms github-app [--org NAME]`, then `agentplaza platforms install <app id>`.

### GitLab and Forgejo: an OAuth app you register

1. On the host (gitlab.com, your GitLab, codeberg.org, your Forgejo), register an OAuth application:
   - **Redirect URI**: the hub's callback, shown on the screen (`<hub URL>/v1/platforms/callback`).
   - **GitLab scopes**: `api` and `read_user`. Forgejo needs no scopes.
   - Leave it confidential (it has a secret).
2. Paste its **client ID** and **secret** into the plaza (the secret goes into the vault; it is never shown again).
3. **Sign in.** The platform asks you to authorize the plaza; the hub uses PKCE, and renews the token when it
   expires.

`agentplaza platforms register gitlab --client-id ID [--host URL]` (the secret is read from the terminal), then
`agentplaza platforms sign-in <app id>`.

### Any of them: a token

Paste a token when you would rather not register an app: a personal access token (your account), a GitLab group
access token or a token limited to one organisation (**organisation**, naming it), a GitLab project access token or a
fine-grained GitHub token for one repository (**one repository**, naming it). The token goes into the vault.

`agentplaza platforms token gitlab --level org --target team/tools` (the token is read from the terminal).

## 2. Choose what the plaza may use

An authorization reaches namespaces (your account, your organisations or groups) and their repositories. Under
**Choose what the plaza may use** you tick, per namespace, **all repositories** or the ones you name. Nothing else can
be connected.

- A GitHub App installation starts with what you gave it on GitHub.
- A sign-in or a user-level token starts with nothing: you choose.
- An organisation- or repository-level token starts with its organisation or its repository.

`agentplaza platforms grant <id> <namespace> --all` or `--repo NAME ...`; `agentplaza platforms ungrant ...`.

## 3. Connect a repository to a project

**Connect to a project** joins a chosen repository to a project's Weft repository:

- If the project's repository has no history yet, the platform's is imported once (every commit and tag; Weft checks
  each commit's files against git's).
- If both have history, the first export stops and shows what differs; you can adopt the platform's commits (they go
  through review like any change).
- **Register the hub's signing key** (on by default) puts the hub's SSH signing key on your account, so exported
  commits and tags show as Verified.
- **Push now** exports what Weft has at once; otherwise the next landing does.

The plaza never creates a repository on a platform: connect one that exists. **Disconnect** stops the export; **Remove**
forgets an authorization and its tokens (its connections stop exporting until another authorization covers them).

`agentplaza platforms connect <id> owner/repo --project pr1`.

## Where things are kept

- Client secrets, the GitHub App's private key and every token are vault entries (binding `hub:git-platforms`).
- The record of apps, authorizations and your choices is `<state>/platforms.json`; it holds no secret.
- Exports are one-way: nothing the platform changes comes back without your adopting it.
