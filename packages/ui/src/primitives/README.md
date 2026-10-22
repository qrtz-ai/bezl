# primitives (shadcn structure on radix, styled by qrtz tokens)

The behaviour layer of @qrtz/ui follows shadcn/ui: each interactive component (Modal, Menu, Tabs, Switch,
Select, Slider) is built on the same Radix primitive shadcn uses (`@radix-ui/react-*`), with shadcn's part
names (Content, Item, Trigger, Thumb...). Visuals stay on our tokens (`tokens.ts`, the `--q-*` vars); the
palette is already shadcn neutral, so the shadcn roles map 1:1:

| shadcn var          | qrtz token            |
| ------------------- | --------------------- |
| background          | color.bg              |
| foreground          | color.text            |
| popover / card      | color.surface / card  |
| muted-foreground    | color.textMuted       |
| accent              | color.accent          |
| border / input      | color.border          |
| ring                | color.ring            |
| primary             | color.primary         |
| destructive         | color.danger          |
| radius              | radius.md (lg for dialogs) |

No Tailwind: see the QRT note in the PR. Overlays render inline (no Portal) so they stay inside the
theme provider and inside an inert, hidden sub app.

Copy stays lowercase (check-titles): every label goes through `lowerTitle` / `toLowerCase`.
