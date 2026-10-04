"""Run independent commands at once and report each one's output whole.

    python tools/parallel.py "python tools/test_a.py" "python tools/test_b.py --flag"

Each argument is one command, split on spaces; a leading `python` is this
interpreter. Commands run concurrently (at most one per CPU), each one's output
is printed in a block when it finishes, and the exit status is 1 if any failed.
Only commands that neither write what another reads nor share an output file
belong in one call.
"""
import concurrent.futures
import os
import subprocess
import sys
import time


def run(command):
    argv = command.split()
    if argv[0] == "python":
        argv[0] = sys.executable
    started = time.monotonic()
    result = subprocess.run(argv, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                            text=True, encoding="utf-8", errors="replace")
    return command, result.returncode, result.stdout, time.monotonic() - started


def main(commands):
    if not commands:
        raise SystemExit(__doc__)
    failed = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=min(len(commands), os.cpu_count() or 4)) as pool:
        for future in concurrent.futures.as_completed([pool.submit(run, c) for c in commands]):
            command, code, output, seconds = future.result()
            status = "ok" if code == 0 else f"FAILED ({code})"
            print(f"--- {command}: {status}, {seconds:.1f}s", flush=True)
            if code or output.strip():
                print(output.rstrip(), flush=True)
            if code:
                failed.append(command)
    if failed:
        print("failed:\n  " + "\n  ".join(failed), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
