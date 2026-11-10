imports.gi.versions.Gio = '2.0';
const { Gio, GLib } = imports.gi;
imports.gi.versions.GioUnix = '2.0';
const { GioUnix } = imports.gi;

const BRIDGE = 'org.luminusos.AuroraShell.Foreground';
const PATH = '/org/luminusos/AuroraShell/Foreground';
const CONSUMER = 'org.luminusos.ForegroundBooster';
const XML = `<node><interface name="${BRIDGE}">
<method name="GetFocus"><arg name="pid" type="u" direction="out"/><arg name="app_id" type="s" direction="out"/></method>
<signal name="FocusChanged"><arg name="pid" type="u"/><arg name="app_id" type="s"/></signal>
</interface></node>`;

const loop = new GLib.MainLoop(null, false);
const conn = Gio.DBus.session;
let owner = null;
let pid = 0;

conn.register_object(PATH, Gio.DBusNodeInfo.new_for_xml(XML).lookup_interface(BRIDGE),
  (c, sender, p, i, method, params, inv) => {
    if (sender !== owner) { inv.return_dbus_error('org.freedesktop.DBus.Error.AccessDenied', 'no'); return; }
    inv.return_value(new GLib.Variant('(us)', [pid, '']));
  }, null, null);
Gio.DBus.session.own_name(BRIDGE, Gio.BusNameOwnerFlags.NONE, null, null);

function emit() {
  if (!owner) return;
  conn.emit_signal(owner, PATH, BRIDGE, 'FocusChanged', new GLib.Variant('(us)', [pid, '']));
  print(`emitted ${pid}`);
}

Gio.DBus.watch_name(Gio.BusType.SESSION, CONSUMER, Gio.BusNameWatcherFlags.NONE,
  (c, n, o) => { owner = o; print(`consumer ${o}`); emit(); },
  () => { owner = null; });

const stdin = new Gio.DataInputStream({ base_stream: new GioUnix.InputStream({ fd: 0 }) });
function readLine() {
  stdin.read_line_async(GLib.PRIORITY_DEFAULT, null, (s, res) => {
    const [line] = s.read_line_finish_utf8(res);
    if (line === null) { loop.quit(); return; }
    pid = parseInt(line.trim());
    emit();
    readLine();
  });
}
readLine();
loop.run();
