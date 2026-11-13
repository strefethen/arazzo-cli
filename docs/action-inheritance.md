# Workflow action inheritance

Steps retain workflow `successActions` and `failureActions`. A step action
replaces a workflow action with the same case-sensitive `name`, after reusable
objects have been resolved. Other workflow actions remain available.

This executor evaluates step actions in their declared order, followed by
unmatched workflow actions in workflow order. This combined ordering is a
repository choice; the specification requires retention and first matching
action execution but does not prescribe this combined ordering.

An exhausted failure retry continues to subsequent actions in that combined
list, including inherited recovery actions. Action debugger checkpoints and
retry counters use indices in the same combined list. The parallel dependency
scheduler also uses that list when inspecting action criteria.
