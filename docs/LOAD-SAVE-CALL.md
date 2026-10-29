# The in-game load call

Found in the Gameface UI export: `E:\FModel\Output\Exports\F1Manager24\Content\UIGameface`.

## The call

``js/project/modules/loadSave/LoadSaveScreen.js``, line 128:

```js
onSelectLoadData = () => {
    if (this.props.inGame != undefined && !this.props.inGame) {
        const slotName = DS.getValueAsString(this.state.selectedSlotCtx, 'slotName');
        (0, Engine_1.sendEvent)('LoadGame', slotName);
        this.setState({ fadeToBlack: true });
    }
    ...
```

And in ``js/common/core/Engine.js``:

```js
function sendEvent(eventName, ...args) {
    engine.trigger(eventName, ...args);
}
```

So the game loads a save by receiving a **named Gameface event**, `LoadGame`, with exactly one
argument: the slot name. `engine.trigger` is Coherent Gameface's bridge into the game, so there is a
matching handler on the game side listening for `LoadGame`. That handler is the thing to find and call
— not `LoadGameFromSlot`, which is the lower-level engine file API and would bypass the game's own
teardown and setup of the world.

## Slot names

From ``js/project/data/Saves.js``:

```js
for (let i = 0; i < 30; i++) {
    const contextID = i == 0 ? 'autoSave' : ('save' + i);
    ...
    preact.h(DataStoreSchema_1.Prop, { id: 'slotName', string: true, generator_value: contextID }),
```

| slot name | file |
|---|---|
| `autoSave` | `autosave.sav` |
| `save1` .. `save29` | `save1.sav` .. `save29.sav` |

That is a clean, small key space — no path handling, no ambiguity about which file is meant. A host
session can name its slot and a client can load that same name.

## Related events on the same screen

Useful because they show the shape of the protocol between the UI and the game:

| event | when |
|---|---|
| `LoadGame` | load the selected slot |
| `NewSave` | create a new save |
| `DeleteData` / `OverwriteData` / `CorruptData` | confirm-dialog outcomes |

## What this does and does not solve

**Solves:** which call to make, and what to pass. No guessing at function addresses or offsets.

**Does not solve:** invoking a Gameface event from outside the UI. Two candidate routes, in order of
preference:

1. **Coherent/Gameface debug** — Gameface exposes a JS execution channel in some builds. If this one
   has it, `engine.trigger('LoadGame', 'autoSave')` can be evaluated directly and the game does the
   rest. Check the `-coherentdebug` style flags or the `UIGameface` config for an inspector port.
2. **Call the game-side `LoadGame` handler directly** — find the UFunction bound to the event and call
   it from UE4SS with the slot string. That means enumerating the object that registers the handler.

Route 1 is much cheaper if it is available and is worth checking first.

## Why this matters beyond loading

The same mechanism is the likely route for the day-advance barrier in
[SAVE-MERGE-PLAN.md](SAVE-MERGE-PLAN.md): the UI already tells the game what to do by name, so pausing
and advancing are events of the same kind. And the slot name is the natural handle for telling a client
"load the host's session" without any file-path logic.
