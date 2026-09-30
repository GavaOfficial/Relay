"""Real relay-capture CLI test: automatic GPU hook selection, HLS, stop and restart."""
import argparse
import ctypes
import json
import mmap
import struct
from pathlib import Path
import queue
import subprocess
import threading
import time


def run():
    parser = argparse.ArgumentParser()
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--zoo", type=Path, required=True)
    parser.add_argument("--exclusive", action="store_true")
    parser.add_argument("--api", choices=["opengl", "dxgi", "d3d9", "vulkan"])
    parser.add_argument("--vsync", action="store_true", help="60 seconds at 60 fps; strict steady-state cadence and one-second segments")
    parser.add_argument("--startup", action="store_true", help="two 60 fps starts: no dropped frames and first segment exactly one second")
    args = parser.parse_args()
    runtime, zoo_path = args.runtime.resolve(), args.zoo.resolve()
    zoo = subprocess.Popen([str(zoo_path)] + (["--exclusive"] if args.exclusive else []) + (["--vsync"] if args.vsync else []), stdout=subprocess.PIPE, text=True)
    hwnd = 0
    capture = None
    try:
        pid, hwnd = map(int, zoo.stdout.readline().split())
        for session in range(1 if args.vsync else 2):
            output = runtime / f"zoo-e2e-{pid}-{session}"
            output.mkdir(parents=True, exist_ok=True)
            cfg = dict(dir=str(output), origin_unix_secs=None, playlist="out.m3u8",
                       source={"Window": {"exe": zoo_path.name}}, fallback_monitor=None,
                       fps=60 if (args.vsync or args.startup) else 30, bitrate_kbps=4000, encoder="auto", game_audio_exe=None,
                       mic=False, mic_gain=1.0, start_segment=0, segment_secs=1, output_height=240)
            config = output / "config.json"
            config.write_text(json.dumps(cfg), encoding="utf-8")
            capture = subprocess.Popen([str(runtime / "relay-capture.exe"), "--config", str(config)],
                                       stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=(output / "stderr.log").open("w", encoding="utf-8"),
                                       text=True, creationflags=subprocess.CREATE_NO_WINDOW)
            messages = queue.Queue()
            def reader(proc):
                for line in proc.stdout:
                    messages.put(json.loads(line))
            thread = threading.Thread(target=reader, args=(capture,), daemon=True)
            thread.start()
            deadline = time.monotonic() + 12
            started = False
            events = []
            while time.monotonic() < deadline:
                try:
                    event = messages.get(timeout=.2)
                except queue.Empty:
                    if capture.poll() is not None:
                        raise RuntimeError(f"recorder exited: {(output / "stderr.log").read_text(encoding="utf-8")}, {events}")
                    continue
                events.append(event)
                if isinstance(event, dict) and "Error" in event:
                    raise RuntimeError(event["Error"])
                if isinstance(event, dict) and "Started" in event:
                    assert event["Started"]["source"] == "hook", events
                    assert event["Started"]["encoder"] != "software", events
                    started = True
                    break
            assert started, events
            if args.api:
                with mmap.mmap(-1, 104, tagname=f"Local\\RelayHook_{pid}", access=mmap.ACCESS_READ) as header:
                    fields = struct.unpack_from("12I", header)
                    assert fields[0] == 0x524c5948 and fields[3] == 0, fields
                    assert fields[10] == {"opengl": 1, "dxgi": 2, "d3d9": 3, "vulkan": 4}[args.api], fields
                    (output / "hook-header.json").write_text(json.dumps(dict(api=fields[10], width=fields[6], height=fields[7], format=fields[9], cpu_payload_capacity=fields[3])), encoding="utf-8")
            time.sleep(60 if args.vsync else 3)
            capture.stdin.write("q\n")
            capture.stdin.flush()
            assert capture.wait(timeout=10) == 0, (output / "stderr.log").read_text(encoding="utf-8")
            thread.join(timeout=2)
            while not messages.empty():
                events.append(messages.get())
            assert not any(isinstance(e, dict) and "Error" in e for e in events), events
            assert not any(isinstance(e, dict) and e.get("SourceChanged", {}).get("source") in ("game", "monitor") for e in events), events
            (output / "events.json").write_text(json.dumps(events, indent=2), encoding="utf-8")
            decoded = subprocess.run(["ffmpeg", "-v", "error", "-i", str(output / "out.m3u8"),
                                      "-vf", "scale=384:240,format=bgra,crop=384:1:0:120", "-f", "rawvideo", "-pix_fmt", "bgra", "pipe:1"], capture_output=True, check=True).stdout
            frame_bytes = 384 * 4
            assert len(decoded) // frame_bytes >= 75, "video too short"
            counts = []
            for start in range(0, len(decoded), frame_bytes):
                value = 0
                for bit in range(24):
                    p = start + ((2 * bit + 1) * 384 // 48) * 4
                    if decoded[p + 2] > decoded[p]:
                        value |= 1 << bit
                counts.append(value)
            # First frames include initialization of the recording encoder. Evaluate
            # steady recording separately; retain the entire video and event log.
            steady = counts[120 if args.vsync else 15:]
            assert all(0 <= b - a <= 1 for a, b in zip(steady, steady[1:])), counts
            assert steady[-1] - steady[0] >= 20, "frozen video"
            if args.startup:
                durations = [float(line.split(":")[1].rstrip(",")) for line in (output / "out.m3u8").read_text().splitlines() if line.startswith("#EXTINF:")]
                assert abs(durations[0] - 1) < .00001, f"first segment: {durations}"
                assert not any(isinstance(e, dict) and "Warning" in e for e in events), events
                assert len(counts) >= 180, f"missing startup frames: {len(counts)}"
                print(f"startup: first segment={durations[0]}s, no warnings or dropped frames")
            if args.vsync:
                assert len(counts) >= 3570, f"only {len(counts)} frames in 60 seconds"
                assert steady[-1] - steady[0] >= 575, "counter stopped progressing"
                durations = [float(line.split(":")[1].rstrip(",")) for line in (output / "out.m3u8").read_text().splitlines() if line.startswith("#EXTINF:")]
                assert len(durations) >= 59, durations
                assert all(abs(d - 1) < .025 for d in durations[2:-1]), durations
                probe = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", "frame=best_effort_timestamp_time", "-of", "json", str(output / "out.m3u8")], capture_output=True, check=True)
                pts = [float(f["best_effort_timestamp_time"]) for f in json.loads(probe.stdout)["frames"]]
                gaps = [b-a for a,b in zip(pts[120:], pts[121:]) if abs(b-a - 1/60) > .002]
                assert not gaps, f"steady-state frame gaps: {gaps[:20]}"
                metrics = [json.loads(line) for line in (output / "capture-timings.jsonl").read_text().splitlines()]
                required = {"AcquireSync", "CopyResource", "Flush", "conversion", "encoder.ProcessInput"}
                assert required <= {m["stage"] for m in metrics}, "missing timing measurements"
                report = dict(frames=len(counts), duration=60, segments=durations, steady_gaps=gaps, api=args.api, vsync=1,
                              max_us={stage: max(m["max_us"] for m in metrics if m["stage"] == stage) for stage in required})
                (output / "vsync-report.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
            print(f"session {session}: source=hook, {len(counts)} decoded frames, consecutive counters, {output}")
            capture = None
            time.sleep(.4)
    finally:
        if capture is not None and capture.poll() is None:
            capture.kill()
            capture.wait()
        if hwnd:
            ctypes.windll.user32.PostMessageW(ctypes.c_void_p(hwnd), 0x10, 0, 0)
        try:
            zoo.wait(timeout=3)
        except subprocess.TimeoutExpired:
            zoo.kill()
            zoo.wait()


if __name__ == "__main__":
    run()
