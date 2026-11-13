---
pull_request: true
---
You are working in a git worktree of your own, on a new branch, to fix what QA found: ${task}

1. Read the pull request and its latest QA comment, which lists the cases tested and what failed. Use `gh pr view <url> --comments`.
2. Reproduce each failure if you can, then fix it, following the conventions of the repository and its tests.
3. Run the repository's checks (build, tests, linters) and fix what fails.
4. Commit, push the branch and open a pull request that explains what QA found and how this fixes it, and links the original pull request.
5. Stay in this session afterwards: you may be asked to resolve conflicts or answer review comments. Once your fix is merged, QA reruns for both pull requests together.
