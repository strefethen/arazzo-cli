# Contributing to arazzo-cli

## About Contributions

I build and maintain arazzo-cli primarily for my own use and share it in the
hope that others find it useful. Bug reports, questions, and suggestions are
welcome, but I don't accept outside code contributions for direct merge.

This is a personal project, and I need to keep its direction and maintenance
manageable. Reviewing and supporting contributed code takes time I don't
have, and I'm responsible for what ships. Sharing the project doesn't mean I
can commit to implementing requests or reviewing every submission.

You're welcome to open a PR to illustrate a bug or a proposed fix. I'll treat
it as a reference, not something to merge. I may use Claude or Codex to help
investigate submissions, then independently decide whether and how to address
them. PRs may be closed without implementation or detailed feedback.

I'm spelling this out so you can decide how much time to spend before doing
the work. A clear bug report and a small reproduction are especially helpful;
there's no need to write a fix.

## Bug Reports

Bug reports are welcome. Please include:

- The command you ran and full `--json` output
- Expected vs. actual behavior
- A minimal `.arazzo.yaml` that reproduces the issue
- Environment details (OS, Rust version, CLI version)
