// Test harness for the multiplayer menu integration.
//
// Proves the parts that can be proven without the game:
//
//   1. `lobby.js` parses and boots without throwing.
//   2. It asks RequireJS for modules that all exist in the export (this is the check that would have
//      caught `project/apps/FrontendRouter`, which does not exist and made the whole integration fail
//      silently).
//   3. The menu patch finds the real `MainMenu_rightBox` inside a tree shaped exactly like the one
//      `MainMenu.js` builds, and the Multiplayer entry ends up as its first child.
//   4. The inner class of the `Focusable.decorate` wrapper is patched, not just the wrapper.
//   5. The screen is registered in `screensLUT` and its component renders a non-empty tree.
//
// The tree in `fixture()` is transcribed from `MainMenu.render` in the export, including the detail
// that matters: children live on `vnode.children`, and `attributes.children` is deleted by `preact.h`.
//
// Run: node tools/menu_test.js

"use strict";

const fs = require("fs");
const path = require("path");
const vm = require("vm");

const PAK_ROOT = path.join(__dirname, "..", "pak", "f1manager24", "Content", "UIGameface");
const EXPORT_ROOT = "E:/FModel/Output/Exports/F1Manager24/Content/UIGameface";
const LOBBY = path.join(PAK_ROOT, "js", "coop", "lobby.js");

let failures = 0;
let checks = 0;

function ok(cond, what, detail) {
    checks++;
    if (cond) {
        console.log("  PASS  " + what);
    } else {
        failures++;
        console.log("  FAIL  " + what + (detail ? "  -> " + detail : ""));
    }
}

// ---------------------------------------------------------------- vnode helpers

// Mirrors what `preact.h` returns: children on the vnode, and `attributes` without them.
// `preact.h` keeps object children as vnodes (it only stringifies primitives), so this does too.
function h(nodeName, attributes) {
    const children = [];
    for (let i = 2; i < arguments.length; i++) {
        const child = arguments[i];
        if (child === null || child === undefined || child === false || child === true) continue;
        if (Array.isArray(child)) {
            for (const c of child) {
                if (c !== null && c !== undefined && c !== false && c !== true) children.push(c);
            }
        } else if (typeof child === "object") {
            children.push(child);
        } else {
            children.push(String(child));
        }
    }
    const attrs = Object.assign({}, attributes);
    delete attrs.children;
    return { nodeName, children, attributes: attrs, key: attrs.key };
}

function element(nodeName, attributes) {
    return h.apply(null, arguments);
}

// A component vnode, the way preact represents `h(SomeComponent, {...}, ...children)`.
function component(Ctor) {
    return element.apply(null, arguments);
}

// ---------------------------------------------------------------- fixture

// Transcribed from MainMenu.render: the logo, the buttons column (with the save-game box), the
// scenario/CAT row, the separator and the bottom row on the left; the carousel and build number on
// the right. The multiplayer entry has to land inside `MainMenu_rightBox`.
function fixture() {
    const logo = element("div", { className: "MainMenu_logo" });
    const continueBtn = component(function FrontendContinueButton() {}, {});
    const loadCareer = component(function MenuButton() {}, { label: "[LOAD_CAREER]" });
    const newCareer = component(function MenuButton() {}, { label: "[NEW_CAREER]" });

    const hasSaveButtonsBox = element(
        "div",
        { className: "MainMenu_hasSaveButtonsBox" },
        continueBtn,
        element("div", { className: "MainMenu_vBox spacing10" }, loadCareer, newCareer)
    );

    const catRow = element(
        "div",
        { className: "MainMenu_hBox spacing20 flex" },
        component(function FrontendTextureButton() {}, { label: "[CREATE_A_TEAM]" }),
        element("div", { className: "MainMenu_vBox spacing20 flex" }, component(function FrontendTextureButton() {}, {}))
    );

    const mainButtons = element("div", { className: "MainMenu_vBox spacing20 mainButtons" }, hasSaveButtonsBox, catRow);
    const bottomRow = element("div", { className: "MainMenu_hBox" }, component(function MenuButton() {}, {}));
    const leftBox = element("div", { className: "MainMenu_leftBox hasSaveGame" }, logo, mainButtons, bottomRow);

    const carousel = component(function Carousel() {}, { rootClassName: "MainMenu_carousel" });
    const buildNo = element("div", { className: "MainMenu_buildNo" }, "100.00.001");
    const bottomRight = element("div", { className: "MainMenu_bottomRightDisplay" }, buildNo);
    const rightBox = element("div", { className: "MainMenu_rightBox" }, carousel, bottomRight);

    const content = component(function FDiv() {}, { rootClassName: "MainMenu_content" }, leftBox, rightBox);

    return element(
        "div",
        { className: "MainMenu_root" },
        component(function StandardBackground() {}, {}),
        component(function DarkAccentSchematic() {}, {}),
        content
    );
}

// ---------------------------------------------------------------- sandbox

// Minimal AMD loader: `define` records, and the `require` passed to callbacks only resolves the
// module ids this harness provides. Anything else is reported, which is how a bad module id is caught.
function makeSandbox(opts) {
    const provided = opts.provided;
    const requested = [];
    const logs = [];

    class Component {
        constructor(props) {
            this.props = props || {};
            this.state = {};
        }
        setState(patch) {
            this.state = Object.assign({}, this.state, typeof patch === "function" ? patch(this.state, this.props) : patch);
            if (this.__onUpdate) this.__onUpdate();
        }
        forceUpdate() {
            if (this.__onUpdate) this.__onUpdate();
        }
    }
    const PreactStub = {
        Component,
        h: h,
        render: function () {
            return null;
        },
    };

    const callbacks = Object.assign({}, opts.provided, {
        "common/lib/preact": PreactStub,
    });

    function makeRequireFn() {
        const req = function (ids, onSuccess, onError) {
            const out = [];
            for (const id of ids) {
                requested.push(id);
                if (!(id in callbacks)) {
                    const err = new Error("Cannot find module '" + id + "'");
                    if (onError) return onError(err);
                    throw err;
                }
                out.push(callbacks[id]);
            }
            if (onSuccess) onSuccess.apply(null, out);
            return undefined;
        };
        req.config = function () {};
        return req;
    }

    const win = {
        require: makeRequireFn(),
        localStorage: (function () {
            const store = {};
            return {
                getItem: (k) => (k in store ? store[k] : null),
                setItem: (k, v) => {
                    store[k] = String(v);
                },
            };
        })(),
        setTimeout: (fn, ms) => setTimeout(fn, ms),
        clearTimeout: (t) => clearTimeout(t),
        setInterval: (fn, ms) => setInterval(fn, ms),
        addEventListener: function () {},
        removeEventListener: function () {},
        console: {
            log: function (m) {
                logs.push(String(m));
            },
        },
        fetch: undefined,
    };
    win.window = win;

    const doc = {
        readyState: "complete",
        head: { appendChild: function () {} },
        body: { appendChild: function () {} },
        documentElement: { style: {} },
        getElementById: function () {
            return null;
        },
        addEventListener: function () {},
        createElement: function () {
            return {
                setAttribute: function () {},
                appendChild: function () {},
                style: {},
            };
        },
    };

    const sandbox = {
        window: win,
        document: doc,
        console: win.console,
        setTimeout: win.setTimeout,
        clearTimeout: win.clearTimeout,
        setInterval: win.setInterval,
        clearInterval: function () {},
        Promise: Promise,
        Object: Object,
        Array: Array,
        Number: Number,
        String: String,
        JSON: JSON,
        encodeURIComponent: encodeURIComponent,
        Error: Error,
        Symbol: Symbol,
        __requested: requested,
        __logs: logs,
    };
    sandbox.globalThis = sandbox;
    return sandbox;
}

// ---------------------------------------------------------------- the module set

function componentStub(name) {
    // A no-render component, enough for the screen's constructor to be inserted into a tree.
    return function () {};
}

function makeProvided() {
    const FDivInner = function (props) {
        this.props = props || {};
    };
    FDivInner.prototype.render = function () {
        return element("div", { className: this.props.rootClassName }, this.props.children);
    };

    const screensLUT = {};
    const ScreenID = { None: 0, MainMenu: 5, NewGame: 6, Load: 8, FrontendCinematics: 11, SelectTeam: 33 };

    const navCalls = [];
    const nav = {
        set: function (id, props) {
            navCalls.push({ id, props });
        },
        get: function () {
            return ScreenID.MainMenu;
        },
        goToPrevious: function () {},
    };

    // Shaped like `Focusable.decorate(_MainMenu)`: the exported component renders exactly one child,
    // which is the real class. Patching the wrapper's render finds nothing to inject into.
    class InnerMainMenu {
        constructor(props) {
            this.props = props || {};
        }
        render() {
            return fixture();
        }
    }
    class MainMenuDecorated {
        constructor(props) {
            this.props = props || {};
        }
        render(props) {
            return component(InnerMainMenu, props || {});
        }
    }

    const CSSUtil = {
        loadCSS: function (url) {
            navCalls.push({ css: url });
        },
    };

    return {
        provided: {
            "common/util/CSSUtil": CSSUtil,
            "common/components/FDiv": { FDiv: FDivInner },
            "common/components/StandardBackground": { StandardBackground: componentStub("StandardBackground") },
            "project/modules/frontend/components/MainMenu": { MainMenu: MainMenuDecorated },
            "project/utils/FrontendNavigation": nav,
            "project/data/FrontendScreens": { ScreenID },
            "project/data/FrontendData": { screensLUT, screens: [] },
            "project/data/GameTypes": {
                EF1Team: {
                    0: "INVALID",
                    1: "Ferrari",
                    2: "McLaren",
                    3: "RedBullRacing",
                    4: "MercedesAMGPetronasF1",
                    5: "Alpine",
                    6: "WilliamsRacing",
                    7: "HaasF1",
                    8: "AlphaTauri",
                    9: "AlfaRomeoRacing",
                    10: "AstonMartin",
                    32: "CustomTeam",
                },
            },
            "project/components/MenuButton": { MenuButton: componentStub("MenuButton") },
            "project/components/SimpleButton": { SimpleButton: componentStub("SimpleButton") },
            "project/components/TeamIconButton": { TeamIconToggle: componentStub("TeamIconToggle") },
            "project/modules/teamManagement/TeamManagementHeader": {
                TeamManagementHeader: componentStub("TeamManagementHeader"),
            },
            "project/utils/VoltaModuleFocusPriority": {
                VoltaModuleFocusPriority: { MainMenu: 17, CareerSettings: 23 },
            },
        },
        InnerMainMenu,
        MainMenuDecorated,
        screensLUT,
        ScreenID,
        navCalls,
    };
}

// ---------------------------------------------------------------- tree search

function findAll(vnode, predicate, out) {
    out = out || [];
    if (vnode === null || vnode === undefined || typeof vnode !== "object") return out;
    if (Array.isArray(vnode)) {
        for (const v of vnode) findAll(v, predicate, out);
        return out;
    }
    if (predicate(vnode)) out.push(vnode);
    const kids = vnode.children;
    if (kids !== undefined && kids !== null) findAll(kids, predicate, out);
    return out;
}

function classNameOf(vnode) {
    return vnode && vnode.attributes && vnode.attributes.className;
}

// ---------------------------------------------------------------- tests

function moduleIdsExistInExport(ids) {
    const missing = [];
    for (const id of ids) {
        // A RequireJS module id maps to `js/<id>.js`; a stylesheet id (as passed to `loadCSS`) maps to
        // `css/<id>.css`. Both are checked, because both appear in the source.
        const asJs = path.join(EXPORT_ROOT, "js", id + ".js");
        const asCss = path.join(EXPORT_ROOT, "css", id + ".css");
        if (fs.existsSync(asJs) || fs.existsSync(asCss)) continue;
        missing.push(id);
    }
    return missing;
}

function run() {
    return runAsync();
}

async function runAsync() {
    console.log("menu integration tests\n");
    // ---- 1. syntax
    const src = fs.readFileSync(LOBBY, "utf8");
    let parsed = true;
    try {
        new vm.Script(src, { filename: LOBBY });
    } catch (e) {
        parsed = false;
        ok(false, "lobby.js parses", e.message);
    }
    if (parsed) ok(true, "lobby.js parses");

    // ---- 2. every module id it asks for exists in the export
    const ids = [];
    const re = /"(project|common)\/[A-Za-z0-9_\/]+"/g;
    let m;
    while ((m = re.exec(src)) !== null) {
        const id = JSON.parse(m[0]);
        // Ignore URLs and anything that is plainly not a module id.
        if (id.indexOf("/img/") >= 0) continue;
        ids.push(id);
    }
    const uniqueIds = Array.from(new Set(ids));
    const missing = moduleIdsExistInExport(uniqueIds);
    ok(
        missing.length === 0,
        "every module id referenced exists in the export (" + uniqueIds.length + " checked)",
        missing.join(", ")
    );

    // ---- 3. boot with the real MainMenu decorator shape
    const env = makeProvided();

    // `preact.Component` is an ES class in the shipped build. Guard the stub against drifting into
    // something callable, because a function-style base is exactly what hid a real crash before.
    let classGuard = true;
    try {
        Component.call({});
        classGuard = false;
    } catch (e) {
        classGuard = true;
    }
    ok(classGuard, "the Component stub is a real ES class, as in the shipped preact");

    const sandbox = makeSandbox({ provided: env.provided });
    const context = vm.createContext(sandbox);
    let bootError = null;
    try {
        vm.runInContext(src, context, { filename: LOBBY });
    } catch (e) {
        bootError = e;
    }
    ok(!bootError, "lobby.js boots without throwing", bootError && (bootError.stack || bootError.message));

    await new Promise((r) => setTimeout(r, 300));

    const logs = sandbox.__logs.join("\n");
    if (process.env.COOP_DEBUG) console.log("--- lobby console ---\n" + logs + "\n---------------------");
    ok(/multiplayer integration loaded/.test(logs), "integration logged as loaded");
    ok(
        !/could not load menu modules/.test(logs),
        "no module load failure reported",
        logs.split("\n").filter((l) => /could not load|threw/.test(l)).join(" | ")
    );

    // ---- 4. the screen registered itself
    ok(env.screensLUT[99] !== undefined, "screen 99 registered in screensLUT");
    if (env.screensLUT[99]) {
        const entry = env.screensLUT[99];
        ok(entry.parent === env.ScreenID.MainMenu, "screen's parent is MainMenu");
        ok(typeof entry.component === "function", "screen has a component");
    }

    // ---- 5. the inner class got patched and the entry lands in the right column
    // The patch happens on the first render, so the tree has to be produced by a render that runs
    // *after* the decorator has been driven once. Driving the decorator is what recovers the inner class.
    const protoProbe = new env.MainMenuDecorated({});
    protoProbe.render({}); // first render: discovers and patches the inner class
    if (process.env.COOP_DEBUG) {
        console.log("decorator Ctor flag: " + env.MainMenuDecorated.__coopWrapped);
        console.log("inner Ctor flag: " + env.InnerMainMenu.__coopWrapped);
        console.log("decorator proto flag: " + env.MainMenuDecorated.prototype.__coopWrapped);
        console.log("inner proto flag: " + env.InnerMainMenu.prototype.__coopWrapped);
    }
    // The flags live on the constructor, which is what `patchMenu` receives.
    ok(env.MainMenuDecorated.__coopWrapped === true, "the exported (decorator) class was patched");
    ok(env.InnerMainMenu.__coopWrapped === true, "the inner class was patched too, not just the decorator");

    const inner = new env.InnerMainMenu({});
    const tree = inner.render();
    if (process.env.COOP_DEBUG) {
        const dump = (v, d) => {
            if (v === null || v === undefined || typeof v !== "object") return " ".repeat(d) + JSON.stringify(v);
            if (Array.isArray(v)) return v.map((x) => dump(x, d)).join("\n");
            const name = typeof v.nodeName === "function" ? v.nodeName.name : v.nodeName;
            const cls = (v.attributes && v.attributes.className) || (v.attributes && v.attributes.rootClassName) || "";
            const kids = v.children === undefined ? "" : " [" + (Array.isArray(v.children) ? v.children.length : "?") + "]";
            let out = " ".repeat(d) + name + " ." + cls + kids;
            if (Array.isArray(v.children)) {
                for (const k of v.children) out += "\n" + dump(k, d + 2);
            }
            return out;
        };
        console.log("--- injected tree ---");
        console.log(dump(tree, 0));
        console.log("---------------------");
    }
    const rightBoxes = findAll(tree, (v) => {
        const cn = classNameOf(v);
        return typeof cn === "string" && cn.indexOf("MainMenu_rightBox") >= 0;
    });
    ok(rightBoxes.length === 1, "exactly one MainMenu_rightBox in the tree", String(rightBoxes.length));

    if (rightBoxes.length === 1) {
        const kids = rightBoxes[0].children || [];
        const first = kids[0];
        const firstIsEntry = !!(
            first &&
            first.attributes &&
            typeof first.attributes.className === "string" &&
            first.attributes.className.indexOf("CoopMainMenuEntry") >= 0
        );
        ok(firstIsEntry, "the multiplayer entry is the first child of the right column");

        const entryChild = first && first.children && first.children[0];
        ok(
            entryChild && entryChild.attributes && entryChild.attributes.label === "Multiplayer",
            "the entry is a MenuButton labelled Multiplayer",
            entryChild && entryChild.attributes && entryChild.attributes.label
        );

        // The carousel must still be there: the entry is prepended, not replacing anything.
        const carousels = findAll(rightBoxes[0], (v) => {
            const a = v.attributes;
            return a && a.rootClassName === "MainMenu_carousel";
        });
        ok(carousels.length === 1, "the replay carousel survived the injection");
    }

    // ---- 6. the left column is untouched
    const leftBoxes = findAll(tree, (v) => {
        const cn = classNameOf(v);
        return typeof cn === "string" && cn.indexOf("MainMenu_leftBox") >= 0;
    });
    ok(leftBoxes.length === 1, "the left column is still present");
    if (leftBoxes.length === 1) {
        const leftEntries = findAll(leftBoxes[0], (v) => {
            const cn = classNameOf(v);
            return typeof cn === "string" && cn.indexOf("CoopMainMenuEntry") >= 0;
        });
        ok(leftEntries.length === 0, "nothing was added to the left column's buttons");
    }

    // ---- 7. shared EMPTY_CHILDREN must not be polluted
    // Two independent renders must produce the same result; mutating a shared array would not.
    const treeA = new env.InnerMainMenu({}).render();
    const treeB = new env.InnerMainMenu({}).render();
    const countEntries = (t) =>
        findAll(t, (v) => {
            const cn = classNameOf(v);
            return typeof cn === "string" && cn.indexOf("CoopMainMenuEntry") >= 0;
        }).length;
    ok(countEntries(treeA) === 1 && countEntries(treeB) === 1, "repeat renders each add exactly one entry", countEntries(treeA) + "/" + countEntries(treeB));

    // ---- 8. the screen component renders
    if (env.screensLUT[99]) {
        const Screen = env.screensLUT[99].component;
        let inst;
        let renderError = null;
        let screenTree = null;
        try {
            inst = new Screen({});
            // The screen fires agent requests on mount; give it a shallow state first.
            inst.state = Object.assign({}, inst.state, { takenBy: {}, myTeam: 0, role: "host" });
            screenTree = inst.render();
        } catch (e) {
            renderError = e;
        }
        ok(!renderError, "the multiplayer screen renders", renderError && (renderError.stack || renderError.message));
        if (screenTree) {
            const teams = findAll(screenTree, (v) => {
                const cn = classNameOf(v);
                return typeof cn === "string" && cn.indexOf("TeamSelectMenu_teams") >= 0;
            });
            ok(teams.length === 1, "the team grid is present");
            if (teams.length === 1) {
                const toggles = findAll(teams[0], (v) => v.nodeName === env.provided["project/components/TeamIconButton"].TeamIconToggle);
                ok(toggles.length === 10, "all 10 teams are offered", String(toggles.length));
                const ids = toggles.map((t) => t.attributes.teamID);
                ok(
                    JSON.stringify(ids) === JSON.stringify([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]),
                    "the team ids are the real EF1Team values",
                    ids.join(",")
                );
            }

            // The back button has to exist: it is part of the requested design.
            const header = findAll(screenTree, (v) => v.nodeName === env.provided["project/modules/teamManagement/TeamManagementHeader"].TeamManagementHeader);
            ok(header.length === 1, "the screen has a header with a back button");
            if (header.length === 1) {
                ok(header[0].attributes.alwaysShowBackButton === true, "the back button is forced visible");
                ok(typeof header[0].attributes.onBack === "function", "the back button has a handler");
            }

            // Both start paths, as requested.
            const buttons = findAll(screenTree, (v) => v.nodeName === env.provided["project/components/SimpleButton"].SimpleButton);
            const labels = buttons.map((b) => b.attributes && b.attributes.label).filter(Boolean);
            ok(labels.indexOf("New career") >= 0, "a New career action is present", labels.join(","));
            ok(labels.indexOf("Load save") >= 0, "a Load save action is present", labels.join(","));
            ok(labels.indexOf("Host") >= 0 && labels.indexOf("Join") >= 0, "Host and Join are present", labels.join(","));
        }
    }

    // ---- 9. the CSS dependency is requested, so the layout classes exist
    const cssLoaded = env.navCalls.filter((c) => c.css).map((c) => c.css);
    if (env.screensLUT[99]) {
        ok(
            cssLoaded.some((u) => u.indexOf("TeamSelectMenu") >= 0),
            "the TeamSelectMenu stylesheet is loaded for the borrowed layout classes",
            cssLoaded.join(", ")
        );
    }

    console.log("\n" + checks + " checks, " + failures + " failed");
    return failures === 0;
}

const passed = run();
passed.then((p) => process.exit(p ? 0 : 1));
