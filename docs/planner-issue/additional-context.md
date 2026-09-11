A stored example, appearance and all:
`https://er-inventory-api.nyasu.business/inventories/c1b606fd1f43d1` comes back
with `bodyType: "B"`, `sliders.cheeks: 148`, `sliders.eyeSize: 242`.

The merge `importState` runs may be why it does not reach the tab. It walks
whichever of the live character and the incoming build has more keys, so a key
the incoming build has and the live character lacks never gets visited. A
character saved once already carries `computed`, and one with an effect toggled
also carries `activeEffects` -- so mine can easily be the shorter of the two
while being the only one with `sliders`.

Nothing about the format needs changing: the bytes I write are the same 264 the
Cosmetics AOB import already reads, from the same starting offset.
