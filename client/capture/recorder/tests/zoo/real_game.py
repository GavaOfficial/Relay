"""Record an already running game with the packaged runtime; retain evidence."""
import argparse
import ctypes
import struct
import json
from pathlib import Path
import queue
import subprocess
import threading
import time


def run():
    parser = argparse.ArgumentParser()
    parser.add_argument("--runtime", type=Path, default=Path("dist/capture-runtime"))
    parser.add_argument("--exe", default="GeometryDash.exe")
    parser.add_argument("--focus-once", action="store_true", help="restore the selected game once before recording")
    parser.add_argument("--pid", type=int, help="collect read-only hook/window diagnostics for this game PID")
    parser.add_argument("--seconds", type=int, default=60)
    parser.add_argument("--height", type=int, default=1080)
    args = parser.parse_args()
    runtime = args.runtime.resolve()
    output = runtime / (Path(args.exe).stem + "-" + time.strftime("%Y%m%d-%H%M%S"))
    output.mkdir(parents=True)
    cfg = dict(dir=str(output), origin_unix_secs=None, playlist="out.m3u8",
               source={"Window": {"exe": args.exe}}, fallback_monitor=None,
               fps=60, bitrate_kbps=12000, encoder="auto", game_audio_exe=None,
               mic=False, mic_gain=1.0, start_segment=0, segment_secs=1,
               output_height=args.height)
    config = output / "config.json"
    config.write_text(json.dumps(cfg, indent=2), encoding="utf-8")
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    user32.GetForegroundWindow.restype = ctypes.c_void_p
    user32.IsIconic.argtypes = [ctypes.c_void_p]
    user32.IsWindow.argtypes = [ctypes.c_void_p]
    user32.GetWindowThreadProcessId.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint32)]
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.OpenFileMappingW.argtypes = [ctypes.c_uint32, ctypes.c_int, ctypes.c_wchar_p]
    kernel32.OpenFileMappingW.restype = ctypes.c_void_p
    kernel32.MapViewOfFile.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint32, ctypes.c_uint32, ctypes.c_size_t]
    kernel32.MapViewOfFile.restype = ctypes.c_void_p
    kernel32.UnmapViewOfFile.argtypes = [ctypes.c_void_p]
    kernel32.CloseHandle.argtypes = [ctypes.c_void_p]
    def snapshot():
        fg = user32.GetForegroundWindow()
        fg_pid = ctypes.c_uint32()
        user32.GetWindowThreadProcessId(fg, ctypes.byref(fg_pid))
        row = dict(seconds=round(time.monotonic()-test_origin, 3), foreground_pid=fg_pid.value)
        if args.pid:
            # Open existing only: never create a mapping that could block the recorder.
            handle = kernel32.OpenFileMappingW(4, 0, f"Local\\RelayHook_{args.pid}")
            if handle:
                view = kernel32.MapViewOfFile(handle, 4, 0, 0, 104)
                if view:
                    data = ctypes.string_at(view, 104)
                    hwnd = struct.unpack_from("Q", data, 72)[0]
                    row.update(hook_frames=struct.unpack_from("Q", data, 56)[0],
                               texture_epoch=struct.unpack_from("Q", data, 88)[0],
                               target_hwnd=hwnd, window_exists=bool(user32.IsWindow(hwnd)),
                               minimized=bool(user32.IsIconic(hwnd)), foreground=fg == hwnd)
                    kernel32.UnmapViewOfFile(view)
                kernel32.CloseHandle(handle)
            else:
                row["mapping_missing"] = True
        return row
    if args.focus_once:
        assert args.pid, "--focus-once requires --pid"
        windows = []
        callback_type = ctypes.WINFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p)
        user32.EnumWindows.argtypes = [callback_type, ctypes.c_void_p]
        user32.IsWindowVisible.argtypes = [ctypes.c_void_p]
        user32.ShowWindow.argtypes = [ctypes.c_void_p, ctypes.c_int]
        user32.SetForegroundWindow.argtypes = [ctypes.c_void_p]
        @callback_type
        def visit(hwnd, _):
            pid = ctypes.c_uint32()
            user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
            if pid.value == args.pid and user32.IsWindowVisible(hwnd):
                windows.append(hwnd)
            return 1
        user32.EnumWindows(visit, None)
        assert windows, "game window not found"
        user32.ShowWindow(windows[0], 9)
        user32.SetForegroundWindow(windows[0])
        time.sleep(1)
        assert user32.GetForegroundWindow() == windows[0], "game did not become foreground"
    test_origin = time.monotonic()
    last_snapshot = test_origin
    last_progress = test_origin
    diagnostic_rows = []
    timed_events = []
    events = []
    messages = queue.Queue()
    with (output / "stderr.log").open("w", encoding="utf-8") as errors:
        capture = subprocess.Popen([str(runtime / "relay-capture.exe"), "--config", str(config)],
                                   stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=errors,
                                   text=True, creationflags=subprocess.CREATE_NO_WINDOW)
        def reader():
            for line in capture.stdout:
                try:
                    messages.put(json.loads(line))
                except ValueError:
                    messages.put({"InvalidOutput": line})
        thread = threading.Thread(target=reader, daemon=True)
        thread.start()
        started = None
        timeout = time.monotonic() + 15
        try:
            while True:
                try:
                    event = messages.get(timeout=.1)
                    events.append(event)
                    timed_events.append(dict(seconds=round(time.monotonic()-test_origin, 3), event=event))
                    print(json.dumps(event), flush=True)
                    if isinstance(event, dict) and "Error" in event:
                        raise RuntimeError(event["Error"])
                    if isinstance(event, dict) and "Started" in event and started is None:
                        started = time.monotonic()
                        assert event["Started"]["source"] == "hook", "hook did not start"
                        assert event["Started"]["encoder"] != "software", "hardware encoder required"
                except queue.Empty:
                    pass
                if time.monotonic() - last_snapshot >= .5:
                    diagnostic_rows.append(snapshot())
                    last_snapshot = time.monotonic()
                    (output / "window-hook-state.json").write_text(json.dumps(diagnostic_rows, indent=2), encoding="utf-8")
                if time.monotonic() - last_progress >= 15 and diagnostic_rows:
                    print("state: " + json.dumps(diagnostic_rows[-1]), flush=True)
                    last_progress = time.monotonic()
                if capture.poll() is not None:
                    raise RuntimeError("recorder exited before end of test")
                if started is None and time.monotonic() > timeout:
                    raise TimeoutError("game not ready")
                if started is not None and time.monotonic() - started >= args.seconds:
                    break
            capture.stdin.write("q\n")
            capture.stdin.flush()
            assert capture.wait(timeout=15) == 0, "recorder failed"
        finally:
            if capture.poll() is None:
                capture.kill()
                capture.wait()
            thread.join(timeout=2)
            while not messages.empty():
                events.append(messages.get())
            (output / "timed-events.json").write_text(json.dumps(timed_events, indent=2), encoding="utf-8")
            (output / "events.json").write_text(json.dumps(events, indent=2), encoding="utf-8")
    playlist = output / "out.m3u8"
    probe = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0",
                            "-show_entries", "frame=best_effort_timestamp_time", "-of", "json", str(playlist)],
                           capture_output=True, check=True)
    pts = [float(f["best_effort_timestamp_time"]) for f in json.loads(probe.stdout)["frames"]]
    durations = [float(line.split(":")[1].rstrip(",")) for line in playlist.read_text().splitlines() if line.startswith("#EXTINF:")]
    metrics = [json.loads(line) for line in (output / "capture-timings.jsonl").read_text().splitlines()]
    # A moving menu should not freeze. Keep detector output for visual confirmation;
    # a genuinely static scene cannot be distinguished from a freeze automatically.
    freeze = subprocess.run(["ffmpeg", "-hide_banner", "-i", str(playlist), "-vf",
                             "scale=160:-2,freezedetect=n=-50dB:d=1", "-an", "-f", "null", "-"],
                            capture_output=True, text=True, check=True)
    (output / "freeze-detection.log").write_text(freeze.stderr, encoding="utf-8")
    subprocess.run(["ffmpeg", "-v", "error", "-i", str(playlist), "-c", "copy", "-movflags", "+faststart",
                    str(output / "recording.mp4")], check=True)
    report = dict(exe=args.exe, requested_seconds=args.seconds, frames=len(pts), segments=durations,
                  steady_frame_gaps=[dict(at=a-pts[0], seconds=b-a) for a,b in zip(pts, pts[1:])
                                     if a-pts[0] >= 2 and abs(b-a-1/60) > .002],
                  potential_freezes=[line for line in freeze.stderr.splitlines() if "lavfi.freezedetect" in line],
                  window_state_changes=[r for i,r in enumerate(diagnostic_rows) if i == 0 or (r.get("foreground"), r.get("minimized")) != (diagnostic_rows[i-1].get("foreground"), diagnostic_rows[i-1].get("minimized"))],
                  warnings=[e["Warning"] for e in events if isinstance(e, dict) and "Warning" in e],
                  max_us={stage: max(m["max_us"] for m in metrics if m["stage"] == stage)
                          for stage in sorted({m["stage"] for m in metrics})})
    (output / "report.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(json.dumps(report, indent=2), flush=True)
    print(f"Evidence: {output}", flush=True)


if __name__ == "__main__":
    run()
