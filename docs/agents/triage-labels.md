# Triage labels

The skills speak in terms of five canonical triage roles. This file maps those roles to the actual label strings used in this repo's issue tracker.

| Label in mattpocock/skills | Label in our tracker | Meaning                                  |
| -------------------------- | -------------------- | ---------------------------------------- |
| `needs-triage`             | `needs-triage`       | Maintainer needs to evaluate this issue  |
| `needs-info`               | `needs-info`         | Waiting on reporter for more information |
| `ready-for-agent`          | `ready-for-agent`    | Fully specified, ready for an AFK agent  |
| `ready-for-human`          | `ready-for-human`    | Requires human implementation            |
| `wontfix`                  | `wontfix`            | Will not be actioned                     |

When a skill mentions a role (e.g. "apply the AFK-ready triage label"), use the corresponding label string from this table.

All five labels exist on the GitHub repo. Edit the right-hand column, and the labels themselves, if the vocabulary ever changes.

## Labels that are not triage states

The repo also carries the GitHub defaults - `bug`, `enhancement`, `documentation`, `question`, `duplicate`, `invalid`, `help wanted`, `good first issue` - plus `accessibility` and `dependencies`. These describe what an issue *is*, not where it sits in triage. Leave them alone when moving an issue through the state machine; in particular do not treat `question` as `needs-info` or `help wanted` as `ready-for-human`.
