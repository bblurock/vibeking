"""Generate Vibeking's original completion sound (MIT). Python standard library only."""
from pathlib import Path
import math
import struct
import wave
sample_rate = 44100
samples = []
for i in range(int(sample_rate * 0.28)):
    t = i / sample_rate
    envelope = min(t / 0.008, 1) * math.exp(-t * 19) * min((0.28 - t) / 0.03, 1)
    value = envelope * (math.sin(2 * math.pi * 880 * t) + 0.35 * math.sin(2 * math.pi * 1320 * t))
    samples.append(struct.pack("<h", round(value * 10000)))
path = Path(__file__).resolve().parents[1] / "public/sounds/complete.wav"
path.parent.mkdir(parents=True, exist_ok=True)
with wave.open(str(path), "wb") as output:
    output.setparams((1, 2, sample_rate, 0, "NONE", "not compressed"))
    output.writeframes(b"".join(samples))
