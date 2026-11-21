"""Drives pcbnew's "Import Netlist" dialog from inside the pcbnew process.

`check_netlist_import.py` installs this file as a pcbnew scripting plugin in a
throwaway KiCad configuration, so pcbnew loads it at startup. It reads a job
description (JSON) from the file named by `NETLIST_IMPORT_JOB`:

    {"board": "...", "netlist": "...", "board_out": "...", "result": "...",
     "screenshot": "..." | null}

and then, through the normal wx event handlers of the dialog:

1. waits until pcbnew has loaded `board` and its libraries, then opens
   File -> Import -> Netlist...,
2. enters the netlist path and selects linking by unique ids (tstamps),
3. clicks "Update PCB" and records the report (full text, and only
   errors/warnings),
4. closes the dialog, saves the board to `board_out` and exits pcbnew.

The result (or the error that stopped the steps) is written to `result` as JSON.
"""

import json
import os
import time
import traceback

import pcbnew
import wx
import wx.html

JOB_FILE = os.environ.get("NETLIST_IMPORT_JOB")
POLL_MS = 200
STEP_TIMEOUT_S = 30
DIALOG_TITLE = "Import Netlist"
MENU_PATH = ["File", "Import", "Netlist..."]
LINK_BY_UUID = 0  # radio box: 0 = unique ids (tstamps), 1 = reference designators


def walk(window):
    yield window
    for child in window.GetChildren():
        yield from walk(child)


def find(root, cls, label=None):
    for window in walk(root):
        if isinstance(window, cls) and (label is None or window.GetLabel() == label):
            return window
    raise LookupError(f"{cls.__name__} {label!r} not found in {root.GetLabel()!r}")


def top_level(predicate):
    return next((w for w in wx.GetTopLevelWindows() if w.IsShown() and predicate(w)), None)


def send(window, event_type):
    event = wx.CommandEvent(event_type, window.GetId())
    event.SetEventObject(window)
    if isinstance(window, (wx.CheckBox, wx.RadioBox)):
        event.SetInt(window.GetValue() if isinstance(window, wx.CheckBox) else window.GetSelection())
    window.GetEventHandler().ProcessEvent(event)


def menu_id(menubar, path):
    menu = menubar.GetMenu(next(i for i in range(menubar.GetMenuCount())
                                if menubar.GetMenuLabelText(i) == path[0]))
    for label in path[1:]:
        item = next(it for it in menu.GetMenuItems() if it.GetItemLabelText() == label)
        if label == path[-1]:
            return item.GetId()
        menu = item.GetSubMenu()


def screenshot(path):
    screen = wx.ScreenDC()
    size = screen.GetSize()
    bitmap = wx.Bitmap(size.width, size.height)
    memory = wx.MemoryDC(bitmap)
    memory.Blit(0, 0, size.width, size.height, screen, 0, 0)
    memory.SelectObject(wx.NullBitmap)
    bitmap.SaveFile(path, wx.BITMAP_TYPE_PNG)


class Driver:
    """Runs the steps as a chain of timers, so the dialog's modal loop keeps running."""

    def __init__(self, job):
        self.job = job
        self.result = {"ok": False}
        self.timers = []

    def finish(self, error=None):
        if error:
            self.result["error"] = error
        with open(self.job["result"], "w", encoding="utf-8") as out:
            json.dump(self.result, out, indent=2)
        os._exit(0)

    def wait(self, condition, then, what):
        deadline = time.monotonic() + STEP_TIMEOUT_S

        def poll():
            try:
                value = condition()
                if value:
                    then(value)
                elif time.monotonic() > deadline:
                    self.finish(f"timed out waiting for {what}")
                else:
                    self.later(poll)
            except Exception:
                self.finish(traceback.format_exc())

        self.later(poll)

    def later(self, function):
        # Keep every timer referenced: an unreferenced wx.CallLater may be garbage
        # collected before it fires (or while it runs). The run is short, so they
        # are never released.
        self.timers.append(wx.CallLater(POLL_MS, function))

    def start(self):
        # The plugin is loaded when the editor is created, before the board file is opened.
        self.wait(self.editor_with_board, self.open_dialog, "the PCB editor to load the board")

    def editor_with_board(self):
        frame = top_level(lambda w: w.GetName() == "PcbFrame")
        board = pcbnew.GetBoard()
        if not (frame and board and board.GetFileName() == self.job["board"]):
            return None
        # KiCad 10 loads the libraries in the background, showing a progress bar in the status
        # bar; footprints are "not found in any libraries" until it is done.
        status = frame.GetStatusBar()
        loading = status and any(isinstance(w, wx.Gauge) and w.IsShown() for w in walk(status))
        return None if loading else frame

    def open_dialog(self, frame):
        self.frame = frame
        self.wait(lambda: top_level(lambda w: w.GetLabel() == DIALOG_TITLE), self.fill,
                  "the Import Netlist dialog")
        # Runs the dialog's modal loop, in which the timers above keep firing. Run it
        # from the main loop rather than nested in this timer's callback.
        wx.CallAfter(frame.ProcessCommand, menu_id(frame.GetMenuBar(), MENU_PATH))

    def fill(self, dialog):
        self.dialog = dialog
        find(dialog, wx.TextCtrl).SetValue(self.job["netlist"])
        link = find(dialog, wx.RadioBox)
        link.SetSelection(LINK_BY_UUID)
        send(link, wx.wxEVT_RADIOBOX)
        self.result["link_method"] = link.GetString(link.GetSelection())
        send(find(dialog, wx.Button, "Update PCB"), wx.wxEVT_BUTTON)
        html = find(dialog, wx.html.HtmlWindow)
        self.wait(lambda: "Total warnings:" in html.ToText() and html, self.read_report,
                  "the import report")

    def read_report(self, html):
        self.result["report"] = html.ToText()
        if self.job.get("screenshot"):
            screenshot(self.job["screenshot"])
        for label in ("Actions", "Infos"):
            box = find(self.dialog, wx.CheckBox, label)
            box.SetValue(False)
            send(box, wx.wxEVT_CHECKBOX)
        self.result["problems"] = html.ToText()
        send(find(self.dialog, wx.Button, "Close"), wx.wxEVT_BUTTON)
        # The dialog is destroyed after closing: look it up again instead of touching it.
        self.wait(lambda: top_level(lambda w: w.GetLabel() == DIALOG_TITLE) is None, self.save,
                  "the dialog to close")

    def save(self, _):
        pcbnew.SaveBoard(self.job["board_out"], pcbnew.GetBoard())
        self.result["ok"] = True
        self.finish()


if JOB_FILE:
    with open(JOB_FILE, encoding="utf-8") as job_file:
        Driver(json.load(job_file)).start()
