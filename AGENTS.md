# Agent rules for qrtz

## Linear tracking (required)

- All qrtz work is tracked in Linear: workspace `qrtz`, team `Qrtz`, key `QRT`, through the Linear connector. Each bot has its own agent account (Beve Bobs, Beve Bozniak, Backend Barry, frontend foo) and is set as the ticket's delegate.
- Before starting work: find or create one ticket for it, delegate it to yourself, and move it to In Progress.
- When it's ready for review: add the commit hash as a ticket comment and move the ticket to In Review.
- Bozniak reviews from the ticket. He moves it to Done if it's cleared, or back to In Progress with his conditions in a comment.
- Put `QRT-<n>` in every commit message.
- Keep it to status and ownership only: no step-by-step comments and no churn.
- Chat is only for real questions, blockers and urgent build items, not hash or status updates.
