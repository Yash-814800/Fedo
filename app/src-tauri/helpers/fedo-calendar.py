#!/usr/bin/env python3
# Fedo's calendar helper: talks to Evolution Data Server (what GNOME Calendar
# uses) through its GObject bindings. One JSON request on stdin, one JSON
# answer on stdout. Embedded in the daemon (calendar.rs) and run with
# `python3 -`; the request arrives as argv[1].
#
#   {"op":"calendars"}
#   {"op":"create","title":..,"start":epoch,"end":epoch,"allDay":bool,
#    "alarms":[-900,..] (seconds from start),"description":..,"calendar":optional uid}
#   {"op":"range","start":epoch,"end":epoch}
#   {"op":"delete","uid":..}
#
# Times are epoch seconds (UTC); all-day events carry the local date of `start`.

import json
import sys
import time
import uuid

import gi

gi.require_version("EDataServer", "1.2")
gi.require_version("ECal", "2.0")
gi.require_version("ICalGLib", "3.0")
from gi.repository import ECal, EDataServer, ICalGLib  # noqa: E402


def out(obj):
    sys.stdout.write(json.dumps(obj))
    sys.exit(0)


def fail(msg):
    out({"ok": False, "error": str(msg)})


def registry():
    return EDataServer.SourceRegistry.new_sync(None)


def pick_source(reg, uid=None):
    if uid:
        s = reg.ref_source(uid)
        if s is not None:
            return s
    return reg.ref_default_calendar()


def client_for(source):
    return ECal.Client.connect_sync(source, ECal.ClientSourceType.EVENTS, 30, None)


def utc(ts):
    return time.strftime("%Y%m%dT%H%M%SZ", time.gmtime(ts))


def esc(s):
    return (str(s).replace("\\", "\\\\").replace(";", "\\;").replace(",", "\\,")
            .replace("\r", "").replace("\n", "\\n"))


def build_ics(req, uid):
    lines = ["BEGIN:VCALENDAR", "VERSION:2.0", "PRODID:-//Fedo//EN", "BEGIN:VEVENT",
             f"UID:{uid}", f"DTSTAMP:{utc(time.time())}", f"SUMMARY:{esc(req['title'])}"]
    if req.get("description"):
        lines.append(f"DESCRIPTION:{esc(req['description'])}")
    if req.get("allDay"):
        d = time.localtime(req["start"])
        day = time.strftime("%Y%m%d", d)
        nxt = time.strftime("%Y%m%d", time.localtime(time.mktime(d) + 86400 + 3600 * 3))
        lines += [f"DTSTART;VALUE=DATE:{day}", f"DTEND;VALUE=DATE:{nxt}"]
    else:
        lines += [f"DTSTART:{utc(req['start'])}", f"DTEND:{utc(req.get('end') or req['start'] + 3600)}"]
    # Offsets in seconds from the start: negative = before it.
    for off in req.get("alarms", []):
        off = int(off)
        lines += ["BEGIN:VALARM", "ACTION:DISPLAY", f"DESCRIPTION:{esc(req['title'])}",
                  f"TRIGGER;RELATED=START:{'-' if off < 0 else ''}PT{abs(off)}S", "END:VALARM"]
    lines += ["END:VEVENT", "END:VCALENDAR"]
    return "\r\n".join(lines) + "\r\n"


def comp_summary(comp):
    return comp.get_summary() or ""


def op_calendars(_req):
    reg = registry()
    default = reg.ref_default_calendar()
    res = []
    for s in reg.list_sources(EDataServer.SOURCE_EXTENSION_CALENDAR):
        ext = s.get_extension(EDataServer.SOURCE_EXTENSION_CALENDAR)
        res.append({"uid": s.get_uid(), "name": s.get_display_name(),
                    "default": default is not None and s.get_uid() == default.get_uid(),
                    "writable": s.get_writable()})
    out({"ok": True, "calendars": res})


def op_create(req):
    reg = registry()
    src = pick_source(reg, req.get("calendar"))
    if src is None:
        fail("no calendar")
    client = client_for(src)
    uid = str(uuid.uuid4())
    comp = ICalGLib.Component.new_from_string(build_ics(req, uid))
    sub = comp.get_first_component(ICalGLib.ComponentKind.VEVENT_COMPONENT)
    new_uid = client.create_object_sync(sub, ECal.OperationFlags.NONE, None)
    uid_out = new_uid[1] if isinstance(new_uid, (tuple, list)) else (new_uid or uid)
    out({"ok": True, "uid": uid_out, "calendar": src.get_display_name(), "calendarUid": src.get_uid()})


def op_range(req):
    reg = registry()
    start, end = int(req["start"]), int(req["end"])
    events = []
    seen = set()
    for s in reg.list_sources(EDataServer.SOURCE_EXTENSION_CALENDAR):
        ext = s.get_extension(EDataServer.SOURCE_EXTENSION_CALENDAR)
        if not ext.get_selected():
            continue
        try:
            client = client_for(s)
        except Exception:
            continue

        def cb(inst, comp, t0, t1, *rest, _s=s):
            return True

        found = []

        def on_instance(*args):
            # (client, comp, instance_start, instance_end, data) across versions
            comp = None
            for a in args:
                if isinstance(a, ICalGLib.Component):
                    comp = a
                    break
            if comp is None:
                for a in args:
                    if hasattr(a, "get_icalcomponent"):
                        comp = a.get_icalcomponent()
                        break
            times = [a for a in args if isinstance(a, ICalGLib.Time)]
            found.append((comp, times))
            return True

        try:
            client.generate_instances_sync(start, end, None, on_instance)
        except Exception as e:  # recurrence expansion unavailable: fall back
            sexp = (f'(occur-in-time-range? (make-time "{utc(start)}") '
                    f'(make-time "{utc(end)}") "UTC")')
            ok, comps = client.get_object_list_as_comps_sync(sexp, None)
            for c in comps:
                found.append((c.get_icalcomponent(), []))
        for comp, times in found:
            if comp is None:
                continue
            uid = comp.get_uid() or ""
            title = comp_summary(comp)
            allday = False
            ts = te = None
            try:
                dts = comp.get_dtstart()
                allday = dts.is_date()
                ts = dts.as_timet_with_zone(dts.get_timezone()) if dts.get_timezone() else dts.as_timet()
                dte = comp.get_dtend()
                te = dte.as_timet_with_zone(dte.get_timezone()) if dte.get_timezone() else dte.as_timet()
            except Exception:
                pass
            if len(times) >= 2:
                try:
                    ts = times[0].as_timet_with_zone(times[0].get_timezone()) if times[0].get_timezone() else times[0].as_timet()
                    te = times[1].as_timet_with_zone(times[1].get_timezone()) if times[1].get_timezone() else times[1].as_timet()
                except Exception:
                    pass
            key = (uid, ts)
            if key in seen:
                continue
            seen.add(key)
            events.append({"uid": uid, "title": title, "start": ts, "end": te,
                           "allDay": allday, "calendar": s.get_display_name()})
    events.sort(key=lambda e: (e["start"] or 0))
    out({"ok": True, "events": events})


def op_delete(req):
    reg = registry()
    for s in reg.list_sources(EDataServer.SOURCE_EXTENSION_CALENDAR):
        try:
            client = client_for(s)
            client.remove_object_sync(req["uid"], None, ECal.ObjModType.THIS,
                                      ECal.OperationFlags.NONE, None)
            out({"ok": True})
        except Exception:
            continue
    fail("not found")


OPS = {"calendars": op_calendars, "create": op_create, "range": op_range, "delete": op_delete}

try:
    request = json.loads(sys.argv[1])
    OPS[request["op"]](request)
except SystemExit:
    raise
except Exception as e:  # noqa: BLE001
    fail(e)
