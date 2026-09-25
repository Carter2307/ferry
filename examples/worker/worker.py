"""A background worker: prints a heartbeat every 2 seconds."""
import os
import time

n = 0
while True:
    n += 1
    print(f"worker {os.environ.get('FERRY_SERVICE_NAME', '?')} heartbeat {n}", flush=True)
    time.sleep(2)
