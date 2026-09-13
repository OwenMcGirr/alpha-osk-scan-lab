"""Run the pinned, real Alpha QML with isolated settings and optional input recording.

The command file is a test-only mechanism for changing this fixture, not an
Alpha IPC implementation. It accepts a small fixed set of fixture operations.
"""
import argparse
import json
import os
from pathlib import Path
import sys
import subprocess
from unittest.mock import MagicMock, patch

parser = argparse.ArgumentParser()
parser.add_argument("--alpha", required=True, type=Path)
parser.add_argument("--work", required=True, type=Path)
parser.add_argument("--live-input", action="store_true")
args = parser.parse_args()
args.work.mkdir(parents=True, exist_ok=True)
os.environ["APPDATA"] = str(args.work / "appdata")
os.environ["QT_QPA_PLATFORM"] = "windows"
os.environ["QT_QUICK_CONTROLS_STYLE"] = "Basic"
sys.path.insert(0, str(args.alpha.resolve()))

from PySide6.QtCore import QSettings, QTimer, QUrl, Qt
from PySide6.QtGui import QAccessible, QGuiApplication
from PySide6.QtWidgets import QApplication
from PySide6.QtQml import QQmlApplicationEngine
from PySide6.QtQuick import QQuickItem
from src.keyboard_app import UIA_APPLICATION_NAME, _KeyboardApplication, _apply_window_flags, _wire_floating_windows
from src.keyboard_bridge import KeyboardBridge
from tests.qml_context import install_context_properties

QApplication.setHighDpiScaleFactorRoundingPolicy(Qt.HighDpiScaleFactorRoundingPolicy.PassThrough)
app = _KeyboardApplication([])
app.setObjectName(UIA_APPLICATION_NAME)
app.setOrganizationName("alpha-osk-scan-lab-fixture")
app.setApplicationName("Alpha-OSK-Scan-Fixture")
QSettings.setDefaultFormat(QSettings.IniFormat)
QSettings.setPath(QSettings.IniFormat, QSettings.UserScope, str(args.work / "settings"))
settings = QSettings()
settings.setValue("ui/savedAutoCheckUpdates", False)
settings.setValue("ui/savedWindowWidth", 1060)
settings.setValue("ui/savedWindowX", 40)
settings.setValue("ui/savedWindowY", 650)
settings.sync()

record_path = args.work / "input-records.jsonl"
alpha_commit = subprocess.check_output(
    ["git", "-C", str(args.alpha), "rev-parse", "HEAD"], text=True
).strip()
def record(name, *values):
    with record_path.open("a", encoding="utf-8") as stream:
        stream.write(json.dumps({"method": name, "args": values}, default=str) + "\n")

if args.live_input:
    bridge = KeyboardBridge()
else:
    synth = MagicMock()
    synth.is_available.return_value = True
    synth.backend_name.return_value = "ScanLabRecorder"
    for method in ["send_key", "send_text", "replace_text", "send_combination", "hold_modifier", "release_modifier"]:
        getattr(synth, method).side_effect = lambda *v, _name=method, **kw: record(_name, *v)
    with patch("src.keyboard_bridge.create_key_synthesizer", return_value=synth):
        bridge = KeyboardBridge()

engine = QQmlApplicationEngine()
install_context_properties(engine, bridge)
engine.load(QUrl.fromLocalFile(str(args.alpha / "qml" / "Main.qml")))
if not engine.rootObjects():
    raise RuntimeError("Alpha QML did not load")
root = engine.rootObjects()[0]
_apply_window_flags(root)
_wire_floating_windows(root)
from src.platform import windows_window
quiet_restore = windows_window.install_quiet_restore(root)
app.keyboard_window = root
QAccessible.setActive(True)
try:
    last_seq = json.loads((args.work / "fixture-command.json").read_text(encoding="utf-8-sig"))["seq"]
except (OSError, ValueError, KeyError):
    last_seq = None

def write_state(seq=None, error=None, attempt=0):
    state = {"seq": seq, "error": error, "live_input": args.live_input,
             "alpha_commit": alpha_commit,
             "bounds_logical": [root.x(), root.y(), root.width(), root.height()],
             "dpi_ratio": root.devicePixelRatio(), "pid": os.getpid()}
    tmp = args.work / "fixture-state.tmp"
    tmp.write_text(json.dumps(state), encoding="utf-8")
    try:
        tmp.replace(args.work / "fixture-state.json")
    except PermissionError:
        # Windows readers can briefly prevent atomic replacement. Retry without
        # blocking Qt's event loop or losing this command's acknowledgement.
        if attempt >= 25:
            raise
        QTimer.singleShot(20, lambda: write_state(seq, error, attempt + 1))

def tick():
    global last_seq
    cmd_path = args.work / "fixture-command.json"
    if not cmd_path.exists():
        return
    try:
        cmd = json.loads(cmd_path.read_text(encoding="utf-8-sig"))
        if cmd["seq"] == last_seq:
            return
        last_seq = cmd["seq"]
        op = cmd["op"]
        if op == "predictions":
            bridge.predictionsChanged.emit(cmd["words"])
        elif op == "geometry":
            root.setPosition(int(cmd["x"]), int(cmd["y"]))
            root.setWidth(int(cmd["width"]))
        elif op == "view":
            allowed = {"compactView", "showNavigation", "showNumpad", "showFunctionRow", "showExtraFunctionRow"}
            for key, value in cmd["properties"].items():
                if key not in allowed:
                    raise ValueError("Unknown fixture property")
                if not root.setProperty(key, bool(value)):
                    raise ValueError("Property absent: " + key)
        elif op == "privacy":
            bridge.setPrivacyMode(bool(cmd["enabled"]))
        elif op == "lock":
            bridge.lockModifier(cmd["modifier"])
        elif op == "visible":
            root.showNormal() if cmd["visible"] else root.showMinimized()
        elif op == "quit":
            app.quit()
        else:
            raise ValueError("Unknown fixture operation")
        QTimer.singleShot(100, lambda: write_state(last_seq))
    except Exception as exc:
        write_state(last_seq, str(exc))

timer = QTimer()
timer.timeout.connect(tick)
timer.start(50)
QTimer.singleShot(500, write_state)
app.aboutToQuit.connect(bridge.shutdown)
print("Alpha fixture ready; " + ("LIVE INPUT" if args.live_input else "recording input only"), flush=True)
sys.exit(app.exec())
