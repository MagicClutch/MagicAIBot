# Autonomous Resource Goals

Read \`instructions/GOALS.md\`, inspect requirements, planner/executor, gathering, crafting, smelting, storage, and inventory. Implement only this prompt; do not explore unloaded terrain, duplicate actions, or create unbounded goals.

## Goal

Support goals such as obtaining cobblestone, an iron pickaxe, bread, or collecting/storing iron ingots through existing capabilities.

Plan deterministically first: check inventory, recent storage when allowed, calculate missing resources, craft/smelt dependencies, loaded gatherable resources, then construct a structured plan. Use AI only if deterministic planning cannot resolve the request.

Use inventory, indexed storage, loaded blocks, drops, crafting, and furnaces only. Optionally retain, deposit labelled storage, approach requester, or use existing safe transfer mechanisms; report unavailable transfer limitations honestly. Track actual item counts.

Test satisfied inventory, withdrawal, gathering, crafting/smelting dependencies, partial completion, no loaded resource, full inventory, cancellation, and replan after state change.
