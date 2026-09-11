The stored document does have the data:
`https://er-inventory-api.nyasu.business/inventories/c1b606fd1f43d1` comes back
with `bodyType: "B"`, `sliders.cheeks: 148`, `sliders.eyeSize: 242`.

The merge `importState` runs looks like the cause. It walks whichever of the two
objects has more keys, so a key the incoming build has and the live character
lacks is never visited. A character that has been saved once carries `computed`,
and one with an effect toggled also carries `activeEffects` -- so an imported
build can be the shorter of the two while being the only one with `sliders`.

The builds come out of a mod of mine that reads the character from the game, but
nothing about the format is unusual: the bytes are the same 264 the Cosmetics AOB
import already reads, from the same offset, and step 1 above reproduces it using
only the planner's own AOB import.
