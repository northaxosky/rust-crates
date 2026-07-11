from hashlib import sha256
from pathlib import Path

root = Path(__file__).parent

djw_commands = [
    "xdelta3-3.1.0-x86_64.exe -f -e -A -N -S djw -W 16384 -s source.bin target.bin xdelta-3.1.0-djw.vcdiff",
    "xdelta3.exe -f -e -a -A -N -S djw -W 16384 -s source.bin target.bin xdelta-3.2.0-djw.vcdiff",
    "xdelta3.exe -f -e -a -A -N -S djw9 -W 16384 -s source.bin target.bin xdelta-3.2.0-djw9.vcdiff",
]

source = bytearray()
seed = b"vcdiff-id2-feasibility-source"
while len(source) < 131072:
    seed = sha256(seed).digest()
    source.extend(seed)

windows = []
for window in range(6):
    payload = bytearray()
    row = 0
    while len(payload) < 16384:
        token = sha256(f"owned-row-{window}-{row}".encode()).hexdigest()[:12]
        line = (
            f"window={window:02}; row={row:05}; class=ID2-FEASIBILITY; "
            f"token={token}; value={(window * 4099 + row * 17) % 100000:05};\n"
        )
        payload.extend(line.encode())
        row += 1
    windows.append(bytes(payload[:16384]))

(root / "source.bin").write_bytes(bytes(source[:131072]))
(root / "target.bin").write_bytes(b"".join(windows))

for command in djw_commands:
    print(command)
