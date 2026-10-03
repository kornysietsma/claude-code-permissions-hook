I wrote this project in December 2025 and I want to update it in several ways:
- I'd like it to support Githhub Copilot - the CLI version for now (we might do IDE version later). Only on MacOS also, and not the cloud versions. This probably entails a name change for the project, and flags for Claude code vs Copilot mode, and docs to make both modes clear. I hope they will have very similar logic and can share most code.
- I'd like to update it to support all the current Claude Code hooks features, I'm not sure what has changed since December 2025.
- I'd like before we make changes, to do a proper senior-engineer review of the codebase - make sure it is clean and sensible and expressive, doesn't have excess comments, follows the engineering-standards skill, has up-to-date dependencies
- I'd like it to be usable for global user-level hooks, and also for per-project hooks - possibly both at once, so if I use Claude in a project, both project-level and user-level hooks will fire (I think this happens anyway if you configure a preToolUse hook for both) - probably in this scenario we'd use one toml config file for user level, and a second one _inside the project_ for project level.  And logging collision would need to be considered.
- I want to consider matching on pretty well any field in the hook payload - I don't really know what the options are, and I think Claude and Copilot might pass different payloads?
- I don't care about backwards compatibility, this doesn't have many users it's more of a proof-of-concept, so I'd be happy to require users to build any configuration from scratch. Use different config file name defaults to avoid people using it with old configs by accident.

I don't know also if this should look at other hooks than preToolUse - but that's probably enough?

The overall goal is twofold:
1. Be able to log exactly what preToolUse hooks are called with - this should be flexible so if the JSON passed changes with new versions, I'll know what is coming and be able to fix this program. This should also log wherever a hook call matches one of our rules so we know what was applied and in what order
2. filter tool usage, using my own regex logic (plus any new matching approaches we think up - I'm not sold on just regexes) to auto-allow some hooks, auto-deny some, seek user permissions on others.

I can't run copilot locally on this machine - I can test things by scping the directory to my work machine but it's a slow back-and-forth so we should verify as much as possible here based on docs, and only test with real copilot when we have to.

I've started this work on a local branch for now, we can push to a remote branch and make a PR when we are near finished.