// SPDX-License-Identifier: BSD-3-Clause
//
// BetterShot Helper: a private D-Bus service inside GNOME Shell for the
// installed BetterShot app.
//
// Wayland keeps apps from reading the screen, the window list, or the global
// pointer, and GNOME's screenshot portal flashes the screen and keeps a
// one-time permission answer. Code inside GNOME Shell has none of those
// limits, so this extension offers BetterShot exactly what it needs and
// nothing more: a silent screenshot of the whole desktop, the windows on the
// current workspace with their positions, a screenshot of one window, and a
// pointer trace for recordings.
//
// Only one program may call it: the BetterShot binary installed by a package
// or `make install` in /usr/bin or /usr/local/bin. Each call compares the
// file the calling process is executing (device and inode of
// /proc/<pid>/exe) with those installed files, so another app cannot pose as
// BetterShot by its name or path. Screenshots are only written inside the
// user's runtime folder. The extension is off on the lock screen, as GNOME
// disables extensions without an unlock-dialog session mode there.

import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import Shell from 'gi://Shell';

import * as Config from 'resource:///org/gnome/shell/misc/config.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const BUS_NAME = 'site.bettershot.Helper';
const OBJECT_PATH = '/site/bettershot/Helper';
const ERROR_PREFIX = 'site.bettershot.Helper.Error';
const API_VERSION = 1;

/** Where packages and `make install` put BetterShot. */
const INSTALLED_PATHS = ['/usr/bin/bettershot', '/usr/local/bin/bettershot'];
/**
 * Extra allowed programs, colon-separated, read from GNOME Shell's own
 * environment. Only whoever starts the shell can set it, so tests and
 * development builds can opt in without opening the service to other apps.
 */
const ALLOWED_PATHS_ENV = 'BETTERSHOT_HELPER_ALLOWED_PATHS';

/** A trace keeps at most this many changes, or runs this long. */
const MAX_TRACE_SAMPLES = 2_000_000;
const MAX_TRACE_SECONDS = 4 * 60 * 60;

/** Window types a person would pick as "a window". */
const PICKABLE_TYPES = [
    Meta.WindowType.NORMAL,
    Meta.WindowType.DIALOG,
    Meta.WindowType.MODAL_DIALOG,
    Meta.WindowType.UTILITY,
];

const HelperIface = `
<node>
  <interface name="site.bettershot.Helper">
    <method name="GetInfo">
      <arg type="a{sv}" direction="out" name="info"/>
    </method>
    <method name="Screenshot">
      <arg type="b" direction="in" name="include_cursor"/>
      <arg type="s" direction="in" name="path"/>
    </method>
    <method name="ListWindows">
      <arg type="aa{sv}" direction="out" name="windows"/>
    </method>
    <method name="ScreenshotWindow">
      <arg type="t" direction="in" name="window_id"/>
      <arg type="b" direction="in" name="include_shadow"/>
      <arg type="s" direction="in" name="path"/>
    </method>
    <method name="StartPointerTrace">
      <arg type="u" direction="in" name="interval_ms"/>
    </method>
    <method name="StopPointerTrace">
      <arg type="a(dddu)" direction="out" name="samples"/>
    </method>
  </interface>
</node>`;

/**
 * Identifies a file by device and inode, following symlinks, so
 * /proc/<pid>/exe names the file a process actually runs.
 *
 * @param {string} path - The file to identify.
 * @returns {string|null} A device:inode key, or null when unreadable.
 */
function fileKey(path) {
    try {
        const info = Gio.File.new_for_path(path).query_info(
            'unix::device,unix::inode', Gio.FileQueryInfoFlags.NONE, null);
        return `${info.get_attribute_uint32('unix::device')}:${info.get_attribute_uint64('unix::inode')}`;
    } catch {
        return null;
    }
}

function allowedPaths() {
    const extra = (GLib.getenv(ALLOWED_PATHS_ENV) ?? '')
        .split(':')
        .filter(path => path.startsWith('/'));
    return [...INSTALLED_PATHS, ...extra];
}

function senderPid(sender) {
    return new Promise((resolve, reject) => {
        Gio.DBus.session.call(
            'org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus',
            'GetConnectionUnixProcessID', new GLib.Variant('(s)', [sender]),
            new GLib.VariantType('(u)'), Gio.DBusCallFlags.NONE, -1, null,
            (connection, result) => {
                try {
                    resolve(connection.call_finish(result).deepUnpack()[0]);
                } catch (e) {
                    reject(e);
                }
            });
    });
}

/**
 * Accepts only absolute, normalized .png paths inside the user's runtime
 * folder, where BetterShot keeps its private working files.
 *
 * @param {string} path - The destination BetterShot asked for.
 * @returns {Gio.FileOutputStream} A stream that replaces the file.
 */
function openDestination(path) {
    const runtime = GLib.get_user_runtime_dir();
    const normalized = GLib.canonicalize_filename(path, null);
    if (!path.startsWith('/') || normalized !== path || !path.endsWith('.png') ||
        !path.startsWith(`${runtime}/`))
        throw new Error('Screenshots can only be written as .png files inside the user runtime folder.');
    return Gio.File.new_for_path(path).replace(
        null, false, Gio.FileCreateFlags.PRIVATE | Gio.FileCreateFlags.REPLACE_DESTINATION, null);
}

function rectTuple(rect) {
    return [rect.x, rect.y, rect.width, rect.height];
}

function pickableWindows() {
    const workspace = global.workspace_manager.get_active_workspace();
    return global.get_window_actors()
        .map(actor => actor.meta_window)
        .filter(window => window && !window.minimized && !window.is_override_redirect() &&
            PICKABLE_TYPES.includes(window.get_window_type()) &&
            window.located_on_workspace(workspace))
        .reverse(); // Topmost first.
}

class HelperService {
    constructor() {
        this._trace = null;
        this._dbusImpl = Gio.DBusExportedObject.wrapJSObject(HelperIface, this);
        this._dbusImpl.export(Gio.DBus.session, OBJECT_PATH);
        this._nameId = Gio.bus_own_name_on_connection(
            Gio.DBus.session, BUS_NAME, Gio.BusNameOwnerFlags.NONE, null, null);
    }

    destroy() {
        this._stopTrace();
        Gio.bus_unown_name(this._nameId);
        this._dbusImpl.unexport();
    }

    /**
     * Runs `work` for an allowed caller and returns its variant, or an
     * AccessDenied or Failed error.
     *
     * @param {Gio.DBusMethodInvocation} invocation - The call.
     * @param {Function} work - Takes {pid, sender}; returns a GLib.Variant.
     */
    _handle(invocation, work) {
        const sender = invocation.get_sender();
        senderPid(sender).then(async pid => {
            const running = fileKey(`/proc/${pid}/exe`);
            const allowed = running !== null && allowedPaths().some(path => fileKey(path) === running);
            if (!allowed) {
                invocation.return_dbus_error(`${ERROR_PREFIX}.AccessDenied`,
                    'Only the installed BetterShot app may use the BetterShot Helper.');
                return;
            }
            invocation.return_value(await work({pid, sender}));
        }).catch(e => {
            invocation.return_dbus_error(`${ERROR_PREFIX}.Failed`, String(e?.message ?? e));
        });
    }

    GetInfoAsync(_params, invocation) {
        this._handle(invocation, () => {
            const display = global.display;
            const monitors = [];
            for (let i = 0; i < display.get_n_monitors(); i++)
                monitors.push([...rectTuple(display.get_monitor_geometry(i)), display.get_monitor_scale(i)]);
            return new GLib.Variant('(a{sv})', [{
                api: new GLib.Variant('u', API_VERSION),
                shell_version: new GLib.Variant('s', Config.PACKAGE_VERSION),
                stage_width: new GLib.Variant('d', global.stage.width),
                stage_height: new GLib.Variant('d', global.stage.height),
                monitors: new GLib.Variant('a(iiiid)', monitors),
            }]);
        });
    }

    /** The whole desktop, every monitor, with no flash and no dialog. */
    ScreenshotAsync([includeCursor, path], invocation) {
        this._handle(invocation, async () => {
            const stream = openDestination(path);
            try {
                const shooter = new Shell.Screenshot();
                await new Promise((resolve, reject) => {
                    shooter.screenshot(includeCursor, stream, (source, result) => {
                        try {
                            source.screenshot_finish(result);
                            resolve();
                        } catch (e) {
                            reject(e);
                        }
                    });
                });
            } finally {
                stream.close(null);
            }
            return new GLib.Variant('()', []);
        });
    }

    /** Windows on the current workspace, topmost first, in stage coordinates. */
    ListWindowsAsync(_params, invocation) {
        this._handle(invocation, ({pid}) => {
            const tracker = Shell.WindowTracker.get_default();
            const windows = pickableWindows()
                .filter(window => window.get_pid() !== pid)
                .map(window => ({
                    id: new GLib.Variant('t', window.get_id()),
                    title: new GLib.Variant('s', window.get_title() ?? ''),
                    app_id: new GLib.Variant('s',
                        tracker.get_window_app(window)?.get_id() ?? window.get_wm_class() ?? ''),
                    frame: new GLib.Variant('(iiii)', rectTuple(window.get_frame_rect())),
                    buffer: new GLib.Variant('(iiii)', rectTuple(window.get_buffer_rect())),
                    monitor: new GLib.Variant('i', window.get_monitor()),
                    focused: new GLib.Variant('b', window.has_focus()),
                }));
            return new GLib.Variant('(aa{sv})', [windows]);
        });
    }

    /**
     * One window's own pixels, even where other windows cover it, as GNOME's
     * screenshot UI captures windows. Without the shadow the image is
     * cropped to the window frame, as macOS window captures are by default.
     */
    ScreenshotWindowAsync([windowId, includeShadow, path], invocation) {
        this._handle(invocation, async () => {
            const window = pickableWindows().find(candidate => candidate.get_id() === Number(windowId));
            if (!window)
                throw new Error('That window is no longer open.');
            const actor = window.get_compositor_private();
            // Hidden actors (the Activities overview, a window mid-animation)
            // have nothing to paint; the caller can crop its still frame.
            if (!actor?.mapped)
                throw new Error('The window is not on screen.');
            const texture = actor.paint_to_content(null)?.get_texture();
            if (!texture)
                throw new Error('The window could not be drawn.');
            const scale = actor.get_resource_scale();
            let [x, y, width, height] = [0, 0, -1, -1];
            if (!includeShadow) {
                const frame = window.get_frame_rect();
                const buffer = window.get_buffer_rect();
                x = Math.round((frame.x - buffer.x) * scale);
                y = Math.round((frame.y - buffer.y) * scale);
                width = Math.round(frame.width * scale);
                height = Math.round(frame.height * scale);
            }
            const stream = openDestination(path);
            try {
                await new Promise((resolve, reject) => {
                    Shell.Screenshot.composite_to_stream(
                        texture, x, y, width, height, scale, null, 0, 0, 1, stream,
                        (_source, result) => {
                            try {
                                Shell.Screenshot.composite_to_stream_finish(result);
                                resolve();
                            } catch (e) {
                                reject(e);
                            }
                        });
                });
            } finally {
                stream.close(null);
            }
            return new GLib.Variant('()', []);
        });
    }

    /**
     * Samples the pointer every `intervalMs` (clamped to 4-100 ms), keeping
     * only changes of position or buttons. Stops when the caller leaves the
     * bus, at the size or time limit, or on StopPointerTrace.
     */
    StartPointerTraceAsync([intervalMs], invocation) {
        this._handle(invocation, ({sender}) => {
            this._stopTrace();
            const started = GLib.get_monotonic_time();
            const trace = {owner: sender, samples: [], last: null, sourceId: 0, watchId: 0};
            const sample = () => {
                const [x, y, mods] = global.get_pointer();
                let buttons = 0;
                if (mods & Clutter.ModifierType.BUTTON1_MASK)
                    buttons |= 1;
                if (mods & Clutter.ModifierType.BUTTON3_MASK)
                    buttons |= 2;
                if (mods & Clutter.ModifierType.BUTTON2_MASK)
                    buttons |= 4;
                const time = (GLib.get_monotonic_time() - started) / 1e6;
                const last = trace.last;
                if (!last || last[1] !== x || last[2] !== y || last[3] !== buttons) {
                    trace.last = [time, x, y, buttons];
                    trace.samples.push(trace.last);
                }
                if (trace.samples.length >= MAX_TRACE_SAMPLES || time > MAX_TRACE_SECONDS) {
                    trace.sourceId = 0;
                    return GLib.SOURCE_REMOVE;
                }
                return GLib.SOURCE_CONTINUE;
            };
            sample();
            const interval = Math.min(Math.max(intervalMs, 4), 100);
            trace.sourceId = GLib.timeout_add(GLib.PRIORITY_HIGH, interval, sample);
            trace.watchId = Gio.bus_watch_name_on_connection(
                Gio.DBus.session, sender, Gio.BusNameWatcherFlags.NONE, null,
                () => {
                    if (this._trace === trace)
                        this._stopTrace();
                });
            this._trace = trace;
            return new GLib.Variant('()', []);
        });
    }

    /** The changes since StartPointerTrace: seconds, x, y, buttons (1 left, 2 right, 4 middle). */
    StopPointerTraceAsync(_params, invocation) {
        this._handle(invocation, ({sender}) => {
            const trace = this._trace;
            const samples = trace && trace.owner === sender ? trace.samples : [];
            if (trace && trace.owner === sender)
                this._stopTrace();
            return new GLib.Variant('(a(dddu))', [samples]);
        });
    }

    _stopTrace() {
        const trace = this._trace;
        this._trace = null;
        if (!trace)
            return;
        if (trace.sourceId)
            GLib.source_remove(trace.sourceId);
        if (trace.watchId)
            Gio.bus_unwatch_name(trace.watchId);
    }
}

export default class BetterShotHelperExtension extends Extension {
    enable() {
        this._service = new HelperService();
    }

    disable() {
        this._service?.destroy();
        this._service = null;
    }
}
