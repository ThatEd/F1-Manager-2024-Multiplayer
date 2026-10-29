// Multiplayer integration for the main menu.
//
// ## How this hooks in (verified against the export, not guessed)
//
// `FrontendApp.js` renders `FrontendModule`, which renders
// `FrontendData.screensLUT[state.selectedMenu].component`. Navigation goes through
// `FrontendNavigation.set(screenID, extraProps)`, which pushes onto a history stack, dispatches
// `onChange` and sends the engine event `FrontendScreenChanged`. A raw `engine.trigger` does
// neither, which is why an earlier attempt produced a black screen.
//
// There is **no** `project/apps/FrontendRouter` module. An earlier version of this file required it,
// so the whole integration failed to load and nothing was patched at all. What exists is the
// `screensLUT` registry above; that is what gets extended now.
//
// ### Getting at the component that actually draws the menu
//
// `MainMenu.js` ends with `exports.MainMenu = Focusable.decorate(_MainMenu)`, so the exported
// component is a *decorator* whose render returns `h(_MainMenu, {...})` - the real class is not
// reachable from outside. Patching the decorator's render and looking for the menu's own boxes inside
// the returned tree finds nothing, because that tree contains only the one child. The inner class is
// recovered from that child (`vnode.nodeName`) and patched once, on the first render.
//
// ### Two facts that shaped the tree walking
//
// 1. `preact.h` **deletes** `attributes.children` and stores them on `vnode.children`
//    (see `common/lib/preact.js`). Reading `props.children` finds nothing.
// 2. `EMPTY_CHILDREN` is a single shared array used by every childless vnode, so children are never
//    mutated in place; the tree is rebuilt instead.
//
// ### Why the labels are literal English
//
// `Localisation.translate` only resolves strings wrapped in `[...]`; anything else passes through
// unchanged (verified in `common/core/Localisation.js`). So no string table ships with the pak.

(function () {
    "use strict";

    var LOBBY_PORT = 45992;
    var BASE = "http://127.0.0.1:" + LOBBY_PORT + "/coop/";

    var KEY_MODE = "coop.mode";
    var KEY_ROLE = "coop.role";
    var KEY_TEAM = "coop.team";

    // Our screen id. The game's enum stops at SelectTeam = 33, so this is clear of it.
    var COOP_SCREEN = 99;

    function log(m) {
        try {
            console.log("[coop] " + m);
        } catch (e) {}
    }

    function lsGet(k, d) {
        try {
            var v = window.localStorage.getItem(k);
            return v === null ? d : v;
        } catch (e) {
            return d;
        }
    }

    function lsSet(k, v) {
        try {
            window.localStorage.setItem(k, String(v));
        } catch (e) {}
    }

    // ------------------------------------------------------------------ agent

    function agent(path, method) {
        return new Promise(function (resolve, reject) {
            var done = false;
            var timer = setTimeout(function () {
                if (!done) {
                    done = true;
                    reject(new Error("no answer from the co-op agent"));
                }
            }, 4000);
            var url = BASE + path;
            var settle = function (fn, arg) {
                if (done) return;
                done = true;
                clearTimeout(timer);
                fn(arg);
            };
            if (typeof fetch === "function") {
                fetch(url, { method: method || "GET", cache: "no-store" })
                    .then(function (r) {
                        if (!r.ok) throw new Error("HTTP " + r.status);
                        return r.json();
                    })
                    .then(function (j) {
                        settle(resolve, j);
                    })
                    .catch(function (e) {
                        settle(reject, e);
                    });
            } else {
                var xhr = new XMLHttpRequest();
                xhr.open(method || "GET", url, true);
                xhr.timeout = 4000;
                xhr.onload = function () {
                    if (xhr.status < 200 || xhr.status >= 300) {
                        return settle(reject, new Error("HTTP " + xhr.status));
                    }
                    try {
                        settle(resolve, JSON.parse(xhr.responseText));
                    } catch (e) {
                        settle(reject, e);
                    }
                };
                xhr.onerror = function () {
                    settle(reject, new Error("no connection to the co-op agent"));
                };
                xhr.ontimeout = function () {
                    settle(reject, new Error("the co-op agent timed out"));
                };
                xhr.send();
            }
        });
    }

    // ------------------------------------------------------------------ css

    // The screen is built from the game's own components and so needs almost no CSS. What is here is
    // placement only: the multiplayer entry in the right-hand column, the selected-mode outline, and
    // the race replay reel shrunk so the two do not compete for the same space.
    //
    // Measured from `Carousel.css`, the reel is `.Carousel_root.withContentBox` at 25.5x16.1rem and its
    // root gets `rootClassName='MainMenu_carousel'`, so it is a child of `.MainMenu_rightBox`. A
    // transform shrinks it visually without touching the game's layout maths, which is the safe way to
    // do this: changing width/height there would fight the component's own sizing.
    //
    // The layout classes it borrows (`TeamSelectMenu_root`, `TeamSelectMenu_content`,
    // `TeamSelectMenu_teams`) come from `css/project/components/Frontend/TeamSelectMenu.css`, which is
    // loaded by the `TeamSelectMenu` module. Nothing on the main-menu path loads that module, so it is
    // required explicitly below rather than assumed.
    var CSS_DEPENDENCIES = ["project/components/Frontend/TeamSelectMenu"];

    var CSS = [
        ".CoopMainMenuEntry{display:flex;flex-direction:column;align-items:flex-start;",
        "margin-bottom:1.1111111111rem;}",
        // Scale, don't resize: transform leaves the component's own layout maths intact.
        ".MainMenu_rightBox .MainMenu_carousel{transform:scale(.6);transform-origin:top right;",
        "margin-top:0;}",
        ".CoopScreen_status{font-size:var(--fontSize14);opacity:.85;line-height:1.4;max-width:34rem;}",
        ".CoopScreen_status.bad{opacity:1;color:#e08a8a;}",
        ".CoopModeColumn{display:flex;flex-direction:column;gap:1.1111111111rem;width:20rem;}",
        ".CoopModeWrap.sel{outline:0.1111111111rem solid rgba(var(--accent-triplet),0.9);",
        "outline-offset:0.2222222222rem;}",
        ".CoopActionsColumn{display:flex;flex-direction:column;gap:.8rem;align-items:flex-start;}",
        ".CoopTeamLabel{font-size:var(--fontSize14);opacity:.8;margin-top:1.7777777778rem;",
        "margin-bottom:.6rem;max-width:70rem;}",
        ".CoopTeamWrap{position:relative;}",
        ".CoopTaken{position:absolute;left:0;right:0;bottom:-1.7rem;text-align:center;",
        "font-size:var(--fontSize14);opacity:.9;}",
    ].join("");

    function injectCss() {
        try {
            if (document.getElementById("coop-screen-css")) return;
            var s = document.createElement("style");
            s.id = "coop-screen-css";
            s.textContent = CSS;
            document.head.appendChild(s);
        } catch (e) {
            log("css not injected: " + e);
        }
    }

    function loadCssDependencies(c) {
        try {
            for (var i = 0; i < CSS_DEPENDENCIES.length; i++) {
                if (c.CSSUtil && typeof c.CSSUtil.loadCSS === "function") {
                    c.CSSUtil.loadCSS(CSS_DEPENDENCIES[i]);
                }
            }
        } catch (e) {
            log("css dependency load failed: " + e);
        }
    }

    // ------------------------------------------------------------------ modules

    var ctx = null;

    // Every path here was checked to exist in the export. The previous list asked for
    // `project/apps/FrontendRouter`, which does not exist, so RequireJS called the error callback and
    // the whole integration gave up silently - which is why no button ever appeared.
    var MODULES = [
        "common/lib/preact",
        "common/util/CSSUtil",
        "common/components/FDiv",
        "common/components/StandardBackground",
        "project/modules/frontend/components/MainMenu",
        "project/utils/FrontendNavigation",
        "project/data/FrontendScreens",
        "project/data/FrontendData",
        "project/data/GameTypes",
        "project/components/MenuButton",
        "project/components/SimpleButton",
        "project/components/TeamIconButton",
        "project/modules/teamManagement/TeamManagementHeader",
        "project/utils/VoltaModuleFocusPriority",
    ];

    function loadModules() {
        return new Promise(function (resolve) {
            if (ctx) return resolve(ctx);
            var attempt = function (tries) {
                if (typeof window.require !== "function") {
                    // `lobby.js` is loaded before `require.js`, so the loader is not there on the
                    // first pass. Wait for it rather than losing the integration to a race.
                    if (tries <= 0) {
                        log("RequireJS never appeared");
                        return resolve(null);
                    }
                    return window.setTimeout(function () {
                        attempt(tries - 1);
                    }, 100);
                }
                try {
                    window.require(
                        MODULES,
                        function (
                            preact,
                            CSSUtil,
                            FDiv,
                            StandardBackground,
                            MainMenu,
                            Nav,
                            Screens,
                            FrontendData,
                            GameTypes,
                            MenuButton,
                            SimpleButton,
                            TeamIconButton,
                            TeamManagementHeader,
                            FocusPriority
                        ) {
                            ctx = {
                                preact: preact,
                                CSSUtil: CSSUtil,
                                FDiv: FDiv.FDiv,
                                StandardBackground: StandardBackground.StandardBackground,
                                MainMenu: MainMenu,
                                /* `FrontendNavigation` exports `set` as a separate named function, NOT as a
                                   `nav.set` method - the module is a bag of functions, not an object with
                                   methods. Passing the namespace therefore produced a `nav` whose `.set` was
                                   undefined, so every `ctx.nav.set(...)` call below threw. It surfaced as
                                   "no multiplayer button" only because the injection guard ran first.

                                   Wrapped into the shape the rest of this file expects, with the namespace
                                   kept on `.module` so anything else it needs is still reachable. */
                                nav: Nav && typeof Nav.set === "function"
                                    ? { set: Nav.set, module: Nav }
                                    : Nav,
                                screens: Screens,
                                frontendData: FrontendData,
                                GameTypes: GameTypes,
                                MenuButton: MenuButton.MenuButton,
                                SimpleButton: SimpleButton.SimpleButton,
                                TeamIconToggle: TeamIconButton.TeamIconToggle,
                                TeamManagementHeader: TeamManagementHeader.TeamManagementHeader,
                                focusPriority: FocusPriority.VoltaModuleFocusPriority,
                            };
                            resolve(ctx);
                        },
                        function (e) {
                            log("could not load menu modules: " + e);
                            log("modules requested: " + MODULES.join(", "));
                            resolve(null);
                        }
                    );
                } catch (e) {
                    log("require threw: " + e);
                    resolve(null);
                }
            };
            attempt(40);
        });
    }

    // ------------------------------------------------------------------ teams

    // Numeric ids from `EF1Team` in `project/data/GameTypes.js`. Kept explicit rather than derived so
    // a rename in a future build cannot silently produce a wrong grid.
    var TEAM_IDS = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];

    function teamLabel(c, id) {
        // `EF1Team[1] === 'Ferrari'`: the enum is emitted in both directions.
        var T = c.GameTypes && c.GameTypes.EF1Team;
        if (T && typeof T[id] === "string") return T[id];
        return "Team " + id;
    }

    function claimsToMap(res) {
        var map = {};
        var list = (res && res.claims) || [];
        for (var i = 0; i < list.length; i++) {
            map[Number(list[i].team)] = list[i].role;
        }
        return map;
    }

    function playerName(role) {
        return role === "client" ? "player 2" : "player 1";
    }

    // ------------------------------------------------------------------ the screen

    var MODES = [
        {
            id: "shared-team",
            label: "Shared team",
            detail: "One garage, two drivers. Each player runs their own car; facilities, research and the budget come from the host.",
        },
        {
            id: "versus",
            label: "Versus",
            detail: "A team each, racing the other player. Shared calendar and results, your own garage.",
        },
        {
            id: "coop-career",
            label: "Co-op career",
            detail: "Two teams, one career. Shared progression and calendar, separate garages.",
        },
    ];

    function modeById(id) {
        for (var i = 0; i < MODES.length; i++) {
            if (MODES[i].id === id) return MODES[i];
        }
        return MODES[0];
    }

    function makeCoopScreen(c) {
        var preact = c.preact;
        var h = preact.h;

        // A real subclass: `preact.Component` is an ES class, so calling it as a function (`Component.call`)
        // throws "Class constructor Component cannot be invoked without 'new'". That mistake was caught by
        // tools/menu_test.js before it ever reached the game.
        class Screen extends preact.Component {
            constructor(props) {
                super(props);
                this.state = {
                    mode: lsGet(KEY_MODE, "shared-team"),
                    role: lsGet(KEY_ROLE, ""),
                    status: "checking the co-op agent...",
                    statusBad: false,
                    armed: false,
                    myTeam: Number(lsGet(KEY_TEAM, "0")) || 0,
                    takenBy: {},
                };
            }
        }

        Screen.prototype.setStatus = function (text, bad) {
            this.setState({ status: text, statusBad: !!bad });
        };

        Screen.prototype.pickMode = function (id) {
            lsSet(KEY_MODE, id);
            this.setState({ mode: id });
        };

        Screen.prototype.pickTeam = function (id) {
            var taken = this.state.takenBy[id];
            if (taken && taken !== this.state.role) return;

            var role = this.state.role || "host";
            lsSet(KEY_TEAM, id);
            var next = {};
            for (var k in this.state.takenBy) {
                if (k !== this.state.myTeam) next[k] = this.state.takenBy[k];
            }
            next[id] = role;
            this.setState({ myTeam: id, takenBy: next });

            // Publish so the other machine shows it as taken. A failed claim must not block the local
            // choice; it only means the other player will not see it.
            agent("claim?team=" + id + "&role=" + role, "POST")
                .then(function (res) {
                    this.setState({ takenBy: claimsToMap(res) });
                }.bind(this))
                .catch(function () {
                    /* local choice stands */
                });
        };

        Screen.prototype.host = function () {
            var self = this;
            lsSet(KEY_ROLE, "host");
            this.setState({ role: "host" });
            this.setStatus("asking the agent...");
            // Open the Steam lobby *here*, on the host tab, rather than at launch.
            //
            // This is what makes a friend's "Join Game" appear at all: Steam only shows a joinable session for
            // a friend who is in a lobby, and this game ships no matchmaking, so the lobby has to be ours. Tied
            // to the tab because Steam's own guidance is to create a lobby only when something needs one.
            //
            // Deliberately not waited on: `host` below reports whether the *sim* is armed, and a Steam failure
            // must not be reported as "cannot host" - the agent endpoint surfaces it separately.
            agent("lobby/set?state=waiting", "POST").catch(function () { /* reported by the status line */ });
            agent("host", "POST")
                .then(function (st) {
                    self.setState({ armed: !!st.armed });
                    self.setStatus(
                        st.armed
                            ? "Hosting. Start or load a save, then advance to the race when both players are on the grid."
                            : "cannot host yet: " + (st.hook_installed ? "the hook has no callback" : "no hook installed"),
                        !st.armed
                    );
                })
                .catch(function (e) {
                    self.setState({ armed: false });
                    self.setStatus(String(e.message || e), true);
                });
        };

        Screen.prototype.join = function () {
            var self = this;
            lsSet(KEY_ROLE, "client");
            this.setState({ role: "client" });
            this.setStatus("asking the agent...");
            // Ask Steam to join the lobby the host is advertising.
            //
            // Two ways a client learns the id, both handled by the agent: a `GameLobbyJoinRequested_t` callback
            // (the friend clicked "Join Game" in the friends list, which Steam records in the agent) and
            // `+connect_lobby <id>` on the command line (an invite accepted while the game was closed).
            //
            // Passing neither is still useful: the agent reports the pending request if one has arrived, so a
            // player can click Join here after clicking Join in Steam. A refusal (wrong build, not a co-op
            // lobby) is surfaced rather than swallowed, because otherwise the client sits waiting for a host
            // that was never going to connect.
            agent("lobby/join", "POST")
                .then(function (r) {
                    if (r && r.error) self.setStatus("Steam: " + r.error, true);
                    else if (r && r.joining) self.setStatus("joined Steam lobby " + r.joining);
                })
                .catch(function () { /* the armed report below still says whether the sim side is ready */ });
            agent("join", "POST")
                .then(function (st) {
                    self.setState({ armed: !!st.armed });
                    self.setStatus(
                        st.armed
                            ? "Joined. Waiting for the host's frames (" + (st.frames_applied || 0) + " applied so far)."
                            : "cannot join yet: no hook installed",
                        !st.armed
                    );
                })
                .catch(function (e) {
                    self.setState({ armed: false });
                    self.setStatus(String(e.message || e), true);
                });
        };

        // Both routes go through the same code the single-player menu uses, so nothing about the
        // career flow is reimplemented here.
        Screen.prototype.newGame = function () {
            try {
                c.nav.set(c.screens.ScreenID.FrontendCinematics);
                var Engine = window.Engine;
                if (Engine && typeof Engine.sendEvent === "function") Engine.sendEvent("FrontendCinematicsPlay");
                this.setStatus("opening a new career");
            } catch (e) {
                this.setStatus("could not open a new career: " + e, true);
            }
        };

        Screen.prototype.loadSave = function () {
            try {
                c.nav.set(c.screens.ScreenID.Load, { onBack: c.nav.goToPrevious, inGame: false });
                this.setStatus("opening the save list");
            } catch (e) {
                this.setStatus("could not open the save list: " + e, true);
            }
        };

        Screen.prototype.goBack = function () {
            // Leaving the host tab tears the lobby down. A lobby that outlived the screen would keep showing
            // "Join Game" to friends for a session nobody is hosting - which is worse than no lobby, because
            // they would connect to nothing and have no way to tell.
            if (this.state && this.state.role === "host") {
                agent("lobby/set?state=idle", "POST").catch(function () { /* nothing to report */ });
            }
            try {
                c.nav.set(c.screens.ScreenID.MainMenu);
            } catch (e) {
                log("back failed: " + e);
            }
        };

        Screen.prototype.componentDidMount = function () {
            var self = this;

            // Opening the co-op screen is the one signal that the player wants co-op now. The bridge stops
            // probing after a few failed sweeps so ordinary single-player play is not paying for an absent
            // server; re-arming here means giving up is invisible - the menu always tries again.
            try {
                if (typeof window.__coopRearm === "function") window.__coopRearm();
            } catch (e) {
                log("re-arm failed: " + e);
            }

            agent("status", "GET")
                .then(function (st) {
                    var armed = !!st.hook_installed;
                    self.setState({ armed: armed });
                    self.setStatus(
                        armed
                            ? "agent ready - hook installed, " + (st.hook_fires || 0) + " calls so far"
                            : "agent present but no hook installed. Restart the game with the agent injected.",
                        !armed
                    );
                })
                .catch(function (e) {
                    self.setState({ armed: false });
                    self.setStatus(
                        "co-op agent not reachable (" +
                            String(e.message || e) +
                            "). Start the game through the co-op launcher.",
                        true
                    );
                });

            agent("teams", "GET")
                .then(function (res) {
                    self.setState({ takenBy: claimsToMap(res) });
                })
                .catch(function () {
                    /* no claims yet is normal */
                });
        };

        Screen.prototype.render = function () {
            var self = this;
            var st = this.state;
            var roleText = st.role === "host" ? "player 1 (host)" : st.role === "client" ? "player 2" : "not chosen yet";

            var modes = MODES.map(function (m) {
                return h(
                    "div",
                    { className: "CoopModeWrap" + (st.mode === m.id ? " sel" : "") },
                    h(c.MenuButton, {
                        label: m.label,
                        icon: "/img/icons/team.svg",
                        outcome: "coopMode_" + m.id,
                        modifiers: "defaultSize crossesBR shadow",
                        onSelect: function () {
                            self.pickMode(m.id);
                        },
                    })
                );
            });

            var teams = TEAM_IDS.map(function (id, index) {
                var who = st.takenBy[id];
                var mine = st.myTeam === id;
                var takenByOther = !!who && who !== st.role && !mine;
                return h(
                    "div",
                    { className: "CoopTeamWrap" },
                    h(c.TeamIconToggle, {
                        teamID: id,
                        toggled: mine,
                        canJoinTeam: !takenByOther,
                        currentTeamID: st.myTeam,
                        onToggle: function () {
                            self.pickTeam(id);
                        },
                        onSelect: function () {
                            self.pickTeam(id);
                        },
                        defaultFocus: index === 0,
                    }),
                    (mine || takenByOther) &&
                        h("div", { className: "CoopTaken" }, mine ? playerName(st.role) : playerName(who))
                );
            });

            var mode = modeById(st.mode);

            return h(
                c.FDiv,
                {
                    rootClassName: "TeamSelectMenu_root",
                    focusModulePriority: c.focusPriority.CareerSettings,
                },
                h(c.StandardBackground, { modifiers: "LGradient noteamGradient" }),
                h(
                    "div",
                    { className: "TeamSelectMenu_content" },
                    h(
                        "div",
                        { className: "TeamSelectMenu_infoContent" },
                        h(
                            "div",
                            { className: "CoopModeColumn" },
                            h("div", { className: "CoopScreen_status" }, "Mode"),
                            modes,
                            h("div", { className: "CoopScreen_status" }, mode.detail)
                        ),
                        h(
                            "div",
                            { className: "TeamSelectMenu_rightContent" },
                            h(
                                "div",
                                { className: "CoopActionsColumn" },
                                h("div", { className: "CoopScreen_status" }, "This machine: " + roleText),
                                h(c.SimpleButton, {
                                    label: "Host",
                                    icon: "/img/icons/team.svg",
                                    // `toggled` is a real modifier in SimpleButton.css; `highlight` is not.
                                    modifiers: st.role === "host" ? "toggled" : "",
                                    onSelect: function () {
                                        self.host();
                                    },
                                }),
                                h(c.SimpleButton, {
                                    label: "Join",
                                    icon: "/img/icons/externalLink.svg",
                                    modifiers: st.role === "client" ? "toggled" : "",
                                    onSelect: function () {
                                        self.join();
                                    },
                                }),
                                h("div", { className: "CoopScreen_status" }, "Start"),
                                h(c.SimpleButton, {
                                    label: "New career",
                                    icon: "/img/icons/newCareer.svg",
                                    onSelect: function () {
                                        self.newGame();
                                    },
                                }),
                                h(c.SimpleButton, {
                                    label: "Load save",
                                    icon: "/img/icons/load.svg",
                                    onSelect: function () {
                                        self.loadSave();
                                    },
                                })
                            )
                        )
                    ),
                    h(
                        "div",
                        null,
                        h(
                            "div",
                            { className: "CoopTeamLabel" },
                            st.myTeam
                                ? "Your team: " +
                                      teamLabel(c, st.myTeam) +
                                      ". Each team can only be taken by one player; the label under a team shows who has it."
                                : "Pick your team. A team already taken is greyed out, with the player's name under it."
                        ),
                        h("div", { className: "TeamSelectMenu_teams" }, teams)
                    )
                ),
                h(
                    "div",
                    { className: "TeamSelectMenu_footerControls" },
                    h("div", { className: "CoopScreen_status" + (st.statusBad ? " bad" : "") }, st.status)
                ),
                h(c.TeamManagementHeader, {
                    breadcrumbs: [
                        {
                            label: "Multiplayer",
                            descr: "Co-op and versus play for two players.",
                        },
                    ],
                    onBack: function () {
                        self.goBack();
                    },
                    alwaysShowBackButton: true,
                    outcome: "MainMenu",
                })
            );
        };

        return Screen;
    }

    // ------------------------------------------------------------------ hot reload

    // Editing UI logic used to cost a repak plus two game restarts, and at least once nothing was
    // tested at all because the new pak was never copied into the install. So the same idea the bridge
    // already uses for its own source is applied here: poll a revision, and when it changes, re-fetch
    // this file and re-evaluate it. Re-running the IIFE re-registers the screen and re-wraps the menu,
    // so both entry points pick up the new code with no restart.
    //
    // Deliberately best-effort: if the server is not running, nothing happens and the pak works.
    var LIVE_FILE = "/lobby.js";
    var LIVE_REV = "/lobby.rev";
    var LIVE_HOST = "127.0.0.1";
    var LIVE_PORT = 45999;
    var LIVE_MS = 3000;

    /*
     * Backoff for the live-reload poll.
     *
     * This used to be an unconditional `setInterval` every 3 s for the life of every page, whether or not
     * anyone was running the command server. Measured cost during ordinary single-player play: 20 HTTP
     * attempts a minute, forever, each a failed connect on a machine with no co-op session. A mod that is
     * installed on both machines and must not disturb normal play cannot do that.
     *
     * New rule:
     *   - while the server answers, poll at LIVE_MS (fast reloads while developing);
     *   - on a miss, back off 6 s, 12 s, 24 s;
     *   - after OFFLINE_GIVE_UP consecutive misses, stop for this document.
     *
     * Nothing is lost: the operator starts the server and reloads the page (or navigates, which re-runs
     * boot) and the poll starts again.
     */
    var OFFLINE_GIVE_UP = 4;
    var liveState = { misses: 0, stopped: false };

    // GET a URL. `onDone(text)` on 200-with-body, `onFail()` on any other outcome. The failure callback
    // is what makes the backoff below possible; without it a dead server was indistinguishable from an
    // unchanged revision.
    function xhrGet(url, onDone, onFail) {
        var settled = false;
        function fail() { if (!settled) { settled = true; if (onFail) onFail(); } }
        try {
            var xr = new XMLHttpRequest();
            xr.open("GET", url, true);
            xr.timeout = 5000;
            xr.onreadystatechange = function () {
                if (xr.readyState !== 4) return;
                if (xr.status !== 200 || !xr.responseText) { fail(); return; }
                settled = true;
                onDone(xr.responseText);
            };
            xr.onerror = fail;
            xr.ontimeout = fail;
            xr.send();
        } catch (e) {
            fail();
        }
    }

    function liveReloadTick() {
        if (liveState.stopped) return;
        xhrGet("http://" + LIVE_HOST + ":" + LIVE_PORT + LIVE_REV + "?t=" + Date.now(), function (text) {
            liveState.misses = 0;
            var rev = String(text || "").trim();
            if (!rev || rev === "missing" || rev === window.__coopLobbyLive.last) return;
            window.__coopLobbyLive.last = rev;
            xhrGet("http://" + LIVE_HOST + ":" + LIVE_PORT + LIVE_FILE + "?t=" + Date.now(), function (src) {
                try {
                    // Compiled before it runs, so a half-written file cannot leave the page with no
                    // multiplayer entry at all.
                    new Function(src)();
                    log("live reload applied (rev " + rev + ")");
                } catch (e) {
                    log("live reload rejected: " + e);
                }
            });
        }, function onMiss() {
            liveState.misses++;
            if (liveState.misses >= OFFLINE_GIVE_UP) {
                liveState.stopped = true;
                log("live reload stopped after " + liveState.misses +
                    " misses (no server; normal play unaffected)");
            }
        });
    }

    function startLiveReload() {
        if (window.__coopLobbyLive) return;
        window.__coopLobbyLive = { last: "" };
        window.__coopLobbyLive.stopped = false;
        var schedule = function () {
            if (liveState.stopped) return;
            var delay = liveState.misses === 0
                ? LIVE_MS
                : LIVE_MS * Math.pow(2, Math.min(liveState.misses, 3));
            setTimeout(function () {
                liveReloadTick();
                schedule();
            }, delay);
        };
        schedule();
    }

    // ------------------------------------------------------------------ screensLUT

    // Registers the screen in the game's own registry, which is what `FrontendModule.render` reads.
    // This is the supported seam: no router module, no monkey-patched router, no DOM overlay.
    function registerScreen() {
        try {
            var lut = ctx.frontendData && ctx.frontendData.screensLUT;
            if (!lut) {
                log("screensLUT missing");
                return false;
            }
            if (lut[COOP_SCREEN]) {
                // A live reload replaces the component; the registry entry has to be refreshed or the
                // next navigation would still build the previous version of the screen.
                lut[COOP_SCREEN].component = makeCoopScreen(ctx);
                log("screen replaced with the reloaded version");
                return true;
            }
            lut[COOP_SCREEN] = {
                id: COOP_SCREEN,
                parent: ctx.screens.ScreenID.MainMenu,
                component: makeCoopScreen(ctx),
            };
            log("screen registered as " + COOP_SCREEN);
            return true;
        } catch (e) {
            log("screen registration failed: " + e);
            return false;
        }
    }

    // ------------------------------------------------------------------ menu patch

    // Rebuilds the vnode tree, prepending `entry` to the children of the first vnode whose className
    // contains `marker`. Never mutates: `EMPTY_CHILDREN` is shared by every childless vnode.
    function prependTo(vnode, marker, entry, h) {
        if (!vnode || typeof vnode !== "object") return vnode;

        if (vnode.pop !== undefined) {
            var out = new Array(vnode.length);
            for (var i = 0; i < vnode.length; i++) out[i] = prependTo(vnode[i], marker, entry, h);
            return out;
        }

        var kids = vnode.children;
        var newKids = kids;
        if (kids && typeof kids === "object") {
            newKids = prependTo(kids, marker, entry, h);
        }

        var attrs = vnode.attributes;
        var cls = attrs && attrs.className;
        var hit = typeof cls === "string" && cls.indexOf(marker) >= 0;

        if (hit) {
            var children = newKids && newKids.pop !== undefined ? newKids : newKids ? [newKids] : [];
            return cloneVnode(vnode, [h("div", { className: "CoopMainMenuEntry" }, entry)].concat(children));
        }

        if (newKids === kids) return vnode;
        return cloneVnode(vnode, newKids);
    }

    function cloneVnode(vnode, children) {
        var copy = {};
        for (var k in vnode) {
            if (Object.prototype.hasOwnProperty.call(vnode, k)) copy[k] = vnode[k];
        }
        copy.children = children;
        return copy;
    }

    // Inserts a real MenuButton into the right-hand column of the real menu, styled by the same
    // modifiers the game uses for its own buttons.
    function patchMenu(menuCtor) {
        try {
            if (!menuCtor || !menuCtor.prototype || !menuCtor.prototype.render) {
                log("menu class has no render");
                return false;
            }
            if (menuCtor.__coopWrapped) {
                // Already wrapped. Installing the previous version's `render` is what makes a live
                // reload swap the behaviour instead of stacking a second wrapper on the first.
                if (menuCtor.__coopOriginalRender) {
                    menuCtor.prototype.render = menuCtor.__coopOriginalRender;
                    menuCtor.__coopWrapped = false;
                } else {
                    return true;
                }
            }

            var h = ctx.preact.h;
            var original = menuCtor.prototype.render;
            menuCtor.__coopOriginalRender = original;

            menuCtor.prototype.render = function (props, state) {
                var tree;
                try {
                    tree = original.call(this, props, state);
                } catch (e) {
                    log("original menu render threw: " + e);
                    throw e;
                }
                try {
                    if (!this.__coopInnerPatched) {
                        // The exported component is a decorator; its single child is the real class.
                        var inner = tree && tree.nodeName;
                        if (
                            inner &&
                            typeof inner === "function" &&
                            inner !== menuCtor &&
                            inner.prototype &&
                            inner.prototype.render
                        ) {
                            this.__coopInnerPatched = patchMenu(inner);
                            log("inner menu class patched: " + this.__coopInnerPatched);
                        }
                    }

                    // Refuse to inject anything unless every ingredient exists.
                    //
                    // `h(undefined, props)` does not throw - preact happily builds a vnode with a null
                    // component - so a missing `MenuButton` would put an unrenderable node into the real main
                    // menu and break it, while this function carried on reporting success. A mod that must
                    // not disturb normal play cannot have that failure mode: no button is a cosmetic loss,
                    // a broken menu is not.
                    //
                    // NOTE: this check must test what is actually used. An earlier version tested
                    // `typeof ctx.nav === "undefined"`, which passes for the module *namespace* - but
                    // `FrontendNavigation` exports `set` as a separate named function, not a `nav.set` method,
                    // so the guard approved an object whose `.set` did not exist and the injection died later.
                    // Testing the callable is the only version that catches that.
                    if (typeof ctx.MenuButton !== "function" || typeof ctx.nav !== "object" ||
                        typeof ctx.nav.set !== "function") {
                        log("menu injection skipped: MenuButton or a callable nav.set unavailable");
                        return tree;
                    }

                    var button = h(ctx.MenuButton, {
                        // Literal, not a loc key: `translate` only resolves `[...]`.
                        label: "Multiplayer",
                        icon: "/img/icons/team.svg",
                        outcome: "coopMultiplayer",
                        modifiers: "defaultSize crossesBR shadow",
                        onSelect: function () {
                            ctx.nav.set(COOP_SCREEN);
                        },
                    });

                    var result = prependTo(tree, "MainMenu_rightBox", button, h);
                    if (result === tree) {
                        log("could not find MainMenu_rightBox in the menu tree");
                    }
                    return result;
                } catch (e) {
                    log("menu injection failed: " + e);
                    return tree;
                }
            };
            menuCtor.__coopWrapped = true;
            log("menu patched");
            return true;
        } catch (e) {
            log("menu patch failed: " + e);
            return false;
        }
    }

    // ------------------------------------------------------------------ visible fallback

    // If the module load or the patch fails, show a plain DOM button rather than nothing. A silent
    // failure is what made the last attempt impossible to diagnose from in-game.
    function fallback(message) {
        try {
            if (document.getElementById("coop-fallback")) return;
            var box = document.createElement("div");
            box.id = "coop-fallback";
            box.setAttribute(
                "style",
                "position:fixed;right:1.2rem;bottom:1.2rem;z-index:99999;display:flex;flex-direction:column;" +
                    "gap:.4rem;font-family:sans-serif;"
            );
            var btn = document.createElement("div");
            btn.textContent = "Multiplayer";
            btn.setAttribute(
                "style",
                "padding:.5rem .9rem;background:#1b1b1b;color:#fff;border:1px solid #777;border-radius:3px;" +
                    "cursor:pointer;font-size:.95rem;"
            );
            btn.onclick = function () {
                try {
                    ctx.nav.set(COOP_SCREEN);
                } catch (e) {
                    log("fallback navigation failed: " + e);
                }
            };
            var note = document.createElement("div");
            note.textContent = message;
            note.setAttribute("style", "color:#e08a8a;font-size:.72rem;max-width:22rem;");
            box.appendChild(btn);
            box.appendChild(note);
            document.body.appendChild(box);
            log("fallback button shown: " + message);
        } catch (e) {
            log("fallback failed: " + e);
        }
    }

    // ------------------------------------------------------------------ boot

    var booted = false;

    function boot() {
        if (booted) return;
        booted = true;
        injectCss();
        startLiveReload();

        loadModules().then(function (c) {
            if (!c) {
                fallback("co-op menu modules failed to load; see the console for [coop] lines");
                return;
            }
            var screenOk = registerScreen();
            loadCssDependencies(c);

            var Mod = c.MainMenu;
            var Ctor = Mod && (Mod.MainMenu || Mod.default);
            if (!Ctor) {
                fallback("MainMenu component not found in the loaded modules");
                return;
            }

            // The menu may already have mounted; patching the prototype still works because the next
            // render goes through it.
            patchMenu(Ctor);

            if (!screenOk) {
                fallback("the multiplayer screen could not be registered");
                return;
            }
        });
    }

    window.CoopLobby = {
        screen: COOP_SCREEN,
        modes: MODES,
        host: function () {
            return agent("host", "POST");
        },
        join: function () {
            return agent("join", "POST");
        },
        status: function () {
            return agent("status", "GET");
        },
        boot: boot,
    };

    if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", boot);
    } else {
        boot();
    }

    log("multiplayer integration loaded");
})();
