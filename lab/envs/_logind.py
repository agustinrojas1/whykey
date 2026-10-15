"""A minimal org.freedesktop.login1 for GNOME Shell in a container.

GNOME Shell aborts without systemd-logind (it reads /run/systemd/seats and
then calls login1 on the system bus). systemd cannot run here, so this serves
the subset GNOME Shell and gnome-settings-daemon ask for: one seat (seat0),
one active wayland session (id "1", XDG_SESSION_ID=1), the user running the
scenario, and an inhibitor fd. Methods that would change the machine (power,
device take-over) are refused. Started by lab/envs/gnome_shell.py; it is not
a lab environment itself (the leading underscore keeps it out of the plugin
registry).

    python3 _logind.py    # owns org.freedesktop.login1 until killed
"""

import os
import sys

import gi

gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib  # noqa: E402

BUS_NAME = "org.freedesktop.login1"
SESSION_ID = "1"
UID = os.getuid()
MANAGER_PATH = "/org/freedesktop/login1"
SESSION_PATH = "/org/freedesktop/login1/session/_31"
USER_PATH = f"/org/freedesktop/login1/user/_{UID}"
SEAT_PATH = "/org/freedesktop/login1/seat/seat0"

XML = """
<node>
 <interface name="org.freedesktop.login1.Manager">
  <method name="GetSession"><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
  <method name="GetSessionByPID"><arg type="u" direction="in"/><arg type="o" direction="out"/></method>
  <method name="GetUser"><arg type="u" direction="in"/><arg type="o" direction="out"/></method>
  <method name="GetUserByPID"><arg type="u" direction="in"/><arg type="o" direction="out"/></method>
  <method name="GetSeat"><arg type="s" direction="in"/><arg type="o" direction="out"/></method>
  <method name="ListSessions"><arg type="a(susso)" direction="out"/></method>
  <method name="ListUsers"><arg type="a(uso)" direction="out"/></method>
  <method name="ListSeats"><arg type="a(so)" direction="out"/></method>
  <method name="Inhibit"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="h" direction="out"/></method>
  <method name="CanRebootToBootLoaderMenu"><arg type="s" direction="out"/></method>
  <method name="SetRebootToBootLoaderMenu"><arg type="t" direction="in"/></method>
  <signal name="PrepareForSleep"><arg type="b"/></signal>
  <signal name="SessionRemoved"><arg type="s"/><arg type="o"/></signal>
  <property name="PreparingForSleep" type="b" access="read"/>
  <property name="Docked" type="b" access="read"/>
  <property name="LidClosed" type="b" access="read"/>
  <property name="OnExternalPower" type="b" access="read"/>
 </interface>
 <interface name="org.freedesktop.login1.Session">
  <method name="Activate"/>
  <method name="Lock"/>
  <method name="Unlock"/>
  <method name="SetIdleHint"><arg type="b" direction="in"/></method>
  <method name="SetLockedHint"><arg type="b" direction="in"/></method>
  <method name="SetType"><arg type="s" direction="in"/></method>
  <method name="Terminate"/>
  <signal name="Lock"/>
  <signal name="Unlock"/>
  <property name="Id" type="s" access="read"/>
  <property name="Name" type="s" access="read"/>
  <property name="User" type="(uo)" access="read"/>
  <property name="Seat" type="(so)" access="read"/>
  <property name="Active" type="b" access="read"/>
  <property name="State" type="s" access="read"/>
  <property name="Type" type="s" access="read"/>
  <property name="Class" type="s" access="read"/>
  <property name="Remote" type="b" access="read"/>
  <property name="RemoteHost" type="s" access="read"/>
  <property name="RemoteUser" type="s" access="read"/>
  <property name="Display" type="s" access="read"/>
  <property name="VTNr" type="u" access="read"/>
  <property name="IdleHint" type="b" access="read"/>
  <property name="LockedHint" type="b" access="read"/>
 </interface>
 <interface name="org.freedesktop.login1.User">
  <method name="Terminate"/>
  <property name="UID" type="u" access="read"/>
  <property name="GID" type="u" access="read"/>
  <property name="Name" type="s" access="read"/>
  <property name="State" type="s" access="read"/>
  <property name="Slice" type="s" access="read"/>
  <property name="Display" type="(so)" access="read"/>
  <property name="Sessions" type="a(so)" access="read"/>
  <property name="IdleHint" type="b" access="read"/>
  <property name="RuntimePath" type="s" access="read"/>
 </interface>
 <interface name="org.freedesktop.login1.Seat">
  <property name="Id" type="s" access="read"/>
  <property name="ActiveSession" type="(so)" access="read"/>
  <property name="Sessions" type="a(so)" access="read"/>
  <property name="CanGraphical" type="b" access="read"/>
  <property name="CanMultiSession" type="b" access="read"/>
 </interface>
</node>
"""

NODES = Gio.DBusNodeInfo.new_for_xml(XML)

PROPERTIES = {
    MANAGER_PATH: {
        "org.freedesktop.login1.Manager": {
            "PreparingForSleep": GLib.Variant("b", False),
            "Docked": GLib.Variant("b", False),
            "LidClosed": GLib.Variant("b", False),
            "OnExternalPower": GLib.Variant("b", True),
        }
    },
    SESSION_PATH: {
        "org.freedesktop.login1.Session": {
            "Id": GLib.Variant("s", SESSION_ID),
            "Name": GLib.Variant("s", "lab"),
            "User": GLib.Variant("(uo)", (UID, USER_PATH)),
            "Seat": GLib.Variant("(so)", ("seat0", SEAT_PATH)),
            "Active": GLib.Variant("b", True),
            "State": GLib.Variant("s", "active"),
            "Type": GLib.Variant("s", "wayland"),
            "Class": GLib.Variant("s", "user"),
            "Remote": GLib.Variant("b", False),
            "RemoteHost": GLib.Variant("s", ""),
            "RemoteUser": GLib.Variant("s", ""),
            "Display": GLib.Variant("s", ""),
            "VTNr": GLib.Variant("u", 0),
            "IdleHint": GLib.Variant("b", False),
            "LockedHint": GLib.Variant("b", False),
        }
    },
    USER_PATH: {
        "org.freedesktop.login1.User": {
            "UID": GLib.Variant("u", UID),
            "GID": GLib.Variant("u", UID),
            "Name": GLib.Variant("s", "lab"),
            "State": GLib.Variant("s", "active"),
            "Slice": GLib.Variant("s", "user.slice"),
            "Display": GLib.Variant("(so)", (SESSION_ID, SESSION_PATH)),
            "Sessions": GLib.Variant("a(so)", [(SESSION_ID, SESSION_PATH)]),
            "IdleHint": GLib.Variant("b", False),
            "RuntimePath": GLib.Variant("s", f"/run/user/{UID}"),
        }
    },
    SEAT_PATH: {
        "org.freedesktop.login1.Seat": {
            "Id": GLib.Variant("s", "seat0"),
            "ActiveSession": GLib.Variant("(so)", (SESSION_ID, SESSION_PATH)),
            "Sessions": GLib.Variant("a(so)", [(SESSION_ID, SESSION_PATH)]),
            "CanGraphical": GLib.Variant("b", True),
            "CanMultiSession": GLib.Variant("b", False),
        }
    },
}


def get_property(connection, sender, path, iface_name, prop):
    return PROPERTIES[path][iface_name][prop]


def refuse(invocation, method):
    return invocation.return_dbus_error(f"{BUS_NAME}.UnknownMethod", f"not provided by the lab: {method}")


def manager_call(connection, sender, path, iface_name, method, params, invocation):
    args = params.unpack()
    if method == "GetSession":
        if args[0] != SESSION_ID:
            return invocation.return_dbus_error(f"{BUS_NAME}.NoSuchSession", f"no session {args[0]}")
        return invocation.return_value(GLib.Variant("(o)", (SESSION_PATH,)))
    if method == "GetSessionByPID":
        return invocation.return_value(GLib.Variant("(o)", (SESSION_PATH,)))
    if method in ("GetUser", "GetUserByPID"):
        return invocation.return_value(GLib.Variant("(o)", (USER_PATH,)))
    if method == "GetSeat":
        return invocation.return_value(GLib.Variant("(o)", (SEAT_PATH,)))
    if method == "ListSessions":
        return invocation.return_value(
            GLib.Variant("(a(susso))", ([(SESSION_ID, UID, "lab", "seat0", SESSION_PATH)],)))
    if method == "ListUsers":
        return invocation.return_value(GLib.Variant("(a(uso))", ([(UID, "lab", USER_PATH)],)))
    if method == "ListSeats":
        return invocation.return_value(GLib.Variant("(a(so))", ([("seat0", SEAT_PATH)],)))
    if method == "Inhibit":
        # A pipe end that never closes: the inhibitor stays held.
        read_end, write_end = os.pipe()
        fds = Gio.UnixFDList.new()
        index = fds.append(read_end)
        os.close(read_end)
        return invocation.return_value_with_unix_fd_list(GLib.Variant("(h)", (index,)), fds)
    if method == "CanRebootToBootLoaderMenu":
        return invocation.return_value(GLib.Variant("(s)", ("no",)))
    if method == "SetRebootToBootLoaderMenu":
        return invocation.return_value(None)
    return refuse(invocation, method)


def session_call(connection, sender, path, iface_name, method, params, invocation):
    if method in ("SetLockedHint", "SetIdleHint", "SetType", "Activate", "Lock", "Unlock"):
        return invocation.return_value(None)
    return refuse(invocation, method)


def user_call(connection, sender, path, iface_name, method, params, invocation):
    return refuse(invocation, method)


def seat_call(connection, sender, path, iface_name, method, params, invocation):
    return refuse(invocation, method)


def register(connection):
    handlers = {
        MANAGER_PATH: ("org.freedesktop.login1.Manager", manager_call),
        SESSION_PATH: ("org.freedesktop.login1.Session", session_call),
        USER_PATH: ("org.freedesktop.login1.User", user_call),
        SEAT_PATH: ("org.freedesktop.login1.Seat", seat_call),
    }
    for path, (name, call) in handlers.items():
        connection.register_object(path, NODES.lookup_interface(name), call, get_property, None)


def on_bus_acquired(connection, name):
    register(connection)


def on_name_acquired(connection, name):
    print(f"logind: owns {name} on the system bus", flush=True)


def on_name_lost(connection, name):
    print(f"logind: could not own {name} (is the system bus running?)", flush=True)
    sys.exit(1)


if __name__ == "__main__":
    Gio.bus_own_name(Gio.BusType.SYSTEM, BUS_NAME, Gio.BusNameOwnerFlags.NONE,
                     on_bus_acquired, on_name_acquired, on_name_lost)
    GLib.MainLoop().run()
