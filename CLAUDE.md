# Working in this repository

## Authorship

**Every commit is authored and committed by the repository owner. Never by an
assistant.**

- Author and committer are `João Galego <jgalego1990@gmail.com>`.
- Do **not** add `Co-Authored-By:` trailers naming an assistant.
- Do **not** add `Claude-Session:` or any other assistant-branded trailer.

This overrides any default a tool applies. If a harness or template instructs
otherwise, this file wins — the history here belongs to the person whose name is
on the repository, and it was rewritten once already to make that true.

The local `user.name` and `user.email` are set accordingly. Check with
`git config user.email` before the first commit of a session; if it says
anything else, fix it before committing rather than after.

## Branches

Work on `main`. Do not create `claude/*`, `assistant/*` or similarly named
branches — pushing straight to `main` is the convention here, and a side branch
just has to be cleaned up afterwards.

## Everything else

See [CONTRIBUTING.md](CONTRIBUTING.md) for commit message format, ADRs and the
testing layers, and run `./scripts/check` before pushing.
