# Schematic Building

Read \`instructions/GOALS.md\`, inspect basic plans, placement/breaking, inventory, and runtime. Implement only this prompt; never download schematics and treat files as untrusted.

## Goal

Build local blueprint files through a normalized internal block-volume format.

Support one documented, tested format first (for example Sponge schematic or project JSON). Enforce file/volume/palette/decompression/coordinate limits, malformed-data handling, and no code execution.

Support translation, rotation, mirroring, origin choice, replacements, ignored blocks, and air handling. Order supports, gravity-sensitive blocks, ordinary blocks, orientation-sensitive blocks, and cleanup; document unsupported specials.

Provide preview dimensions, counts, materials, unsupported blocks, conflicts, and estimate; require explicit build after preview. Test parser, malformed input, limits, palette mapping, transforms, materials, order, and cancellation.
