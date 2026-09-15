#!/usr/bin/env python3
"""GemmaSession — stateful, KV-cache-reusing Gemma transcription for MLX.

The streaming core of the sidecar. Holds ONE persistent KV cache for a recording
and, on each `feed`, prefills only the NEW audio tokens — never re-prefilling the
settled head. Mechanism proven in sidecar_proto.py; append-encoding behaviour
characterised in spike_append.py.

Per-tick flow (`feed`):
  1. Append the new audio to the running buffer.
  2. Re-encode the WHOLE buffer to token embeddings (encoder is causal + cheap,
     ~2ms/s; this sidesteps manual overlap bookkeeping — the prefix is stable per
     spike_append S1). `get_input_embeddings` does encode + splice + per-layer.
  3. Commit new audio tokens [committed : audio_end - keepback] into the PERSISTENT
     cache (hold the last `keepback` tokens — the conv cut-edge is unstable until
     more audio arrives, spike_append S1).
  4. Partial transcript: prefill the remainder (held-back audio + the closing
     `<end_of_audio><end_of_turn>...<start_of_turn>model`) into the cache, greedy
     decode, then TRIM the cache back to `committed` so the next tick continues
     cleanly. (Trim verified consistent for our sequence lengths.)

`finish` commits everything (no keepback) and returns the authoritative transcript.

Tokenizer quirks handled: stop ids are {<eos>=1, <end_of_turn>=106} (this
tokenizer mis-maps <end_of_turn> to 3); logits are bf16.
"""
import os
import re
import tempfile
from difflib import SequenceMatcher
import numpy as np
import mlx.core as mx
import soundfile as sf
from mlx_vlm.prompt_utils import apply_chat_template
from mlx_vlm.utils import prepare_inputs
from mlx_vlm.models import cache as cache_mod
from mlx_lm.models.cache import trim_prompt_cache, can_trim_prompt_cache

SR = 16000
DEFAULT_PROMPT = (
    "Transcribe the following speech into text. Output only the transcription."
)
STOP_IDS = {1, 106}  # <eos>, <end_of_turn>
# When there's too little speech yet, Gemma drifts into a reasoning monologue
# ("no audio provided…") that opens with the channel token (id 100). Treat that
# first token as "not ready" and keep the last good partial instead of emitting
# the hallucination.
CHANNEL_ID = 100
# The audio encoder produces 0 tokens (and crashes with a reshape error) for an
# empty/sub-frame buffer. Below this many 16 kHz samples there's nothing to
# transcribe — skip the model entirely and return the last good text.
MIN_AUDIO_SAMPLES = 1600  # 0.1 s @ 16 kHz

# --- Silence gate -------------------------------------------------------------
# Gemma is an LLM, not an ASR head: handed a window with NO speech in it, it
# doesn't return empty — it invents a short random phrase ("好的。", "Okay." …).
# So never decode a window unless a cheap frame-energy VAD says speech exists:
# enough frames must rise above BOTH an absolute floor and the window's own
# noise floor. The loud-RMS cap keeps continuously-loud speech (no quiet frames
# to form a floor) from failing the relative test.
_VAD_FRAME = 480        # 30 ms @ 16 kHz
_VAD_ABS_RMS = 0.0045   # absolute frame-RMS floor (~ -47 dBFS)
_VAD_SNR = 2.5          # speech frames must be this far above the noise floor
_VAD_LOUD_RMS = 0.04    # frames above this (~ -28 dBFS) always count as speech
_VAD_MIN_FRAMES = 4     # >= 120 ms of qualifying frames to count as speech


def _has_speech(audio):
    """True if `audio` (f32 mono @ 16 kHz) plausibly contains any speech."""
    n = len(audio) // _VAD_FRAME
    if n == 0:
        return False
    rms = np.sqrt((audio[: n * _VAD_FRAME].reshape(n, _VAD_FRAME) ** 2).mean(axis=1))
    floor = np.percentile(rms, 20)
    thresh = max(_VAD_ABS_RMS, min(floor * _VAD_SNR, _VAD_LOUD_RMS))
    return int((rms >= thresh).sum()) >= _VAD_MIN_FRAMES


# --- Greedy-decode repetition guard ------------------------------------------
# Decoding is pure argmax with no sampling-time repetition penalty, so on long or
# ambiguous audio (music, humming, sustained noise) Gemma degenerates into a
# token loop — the "go a go a go" / "you, you, you ×9" runaway seen on a 348 s
# recording. Detect when the emitted tail is a short block repeated REP_LIMIT+
# times and stop, keeping the block ONCE (a genuine repeat survives; the runaway
# is cut). REP_LIMIT is set high enough that emphatic human repetition ("no no
# no no") is preserved — only a degenerate loop reaches it.
_REP_MAX_PERIOD = 4   # detect cycles up to this many tokens long
_REP_LIMIT = 5        # this many back-to-back repeats of a block == degeneration


def _runaway_cut(out):
    """If `out` (token ids) ends in a length-`p` block repeated >= _REP_LIMIT
    times, return the index to truncate to (keeping the block once); else None."""
    n = len(out)
    for p in range(1, _REP_MAX_PERIOD + 1):
        if n < p * _REP_LIMIT:
            continue
        block = out[-p:]
        reps, i = 1, n - 2 * p
        while i >= 0 and out[i : i + p] == block:
            reps += 1
            i -= p
        if reps >= _REP_LIMIT:
            return n - (reps - 1) * p  # keep the first block, drop the repeats
    return None

# --- Sliding-window caching -------------------------------------------------
# Gemma 4 has two hard audio limits the session must stay under:
#   * the processor truncates audio to 30 s / 750 tokens (anything longer is
#     silently dropped from the final), and
#   * the LLM's local-attention layers use a RotatingKVCache that wraps at
#     ~512 tokens (~20 s of audio), after which the incremental partial path
#     can't trim back and decodes corrupt (the live preview garbles / drifts
#     into a random language).
# So the session never lets the ACTIVE window exceed WINDOW_COMMIT_S: when it
# fills, the window is decoded from a fresh cache (clean, well under both caps),
# its text is committed, and the window slides forward keeping OVERLAP_S of audio
# as left context. The full transcript = committed text + current-window partial,
# de-duplicated at the seam (LocalAgreement-style; see _stitch).
WINDOW_COMMIT_S = 18  # slide once the active window reaches this length
OVERLAP_S = 4         # audio carried into the next window as left context
#   OVERLAP_S must be >= 2 s: the Conformer encoder produces garbage for the
#   first ~2 s of a window with no preceding audio (FINDINGS S2/S3). The extra
#   margin also gives the seam enough shared words to de-duplicate reliably.

# CJK / kana / fullwidth ranges — used to decide word-spacing at a join and to
# recognise spaceless scripts (Chinese has no word delimiters, so the seam merge
# below works at the CHARACTER level rather than splitting on spaces).
_CJK = re.compile(
    r"[぀-ヿ㐀-䶿一-鿿豈-﫿＀-￯]"
)
# Leading punctuation/space to trim off the remainder after a seam cut.
_LEAD_PUNCT = " \t\r\n,.;:!?、。，；：！？“”\"'"
_SEAM_WIN = 140  # chars of committed-tail / new-head compared for the overlap


def _join(a, b):
    """Concatenate, inserting a space ONLY between two space-delimited tokens.
    CJK boundaries (either side) get no space — Chinese text isn't space-split."""
    if not a:
        return b
    if not b:
        return a
    if _CJK.search(a[-1]) or _CJK.search(b[0]):
        return a + b
    return a + " " + b


def _norm_chars(s):
    """Char stream for seam comparison: lowercased alphanumerics + CJK (one unit
    per char), punctuation/space dropped, with each unit's index into raw `s`.
    Works for both spaced (English) and spaceless (CJK) scripts."""
    chars, raw = [], []
    for i, ch in enumerate(s):
        c = ch.lower()
        if c.isalnum():  # True for latin alnum AND CJK ideographs/kana
            chars.append(c)
            raw.append(i)
    return chars, raw


def _stitch(committed, new):
    """Append `new` onto `committed`, dropping the text they share at the seam.

    The active window overlaps the previous one by OVERLAP_S of audio, so a fresh
    window's transcript re-covers the tail of what's already committed. Operating
    on a normalised CHARACTER stream (so it works for spaceless CJK as well as
    English), find the run where committed's tail aligns with new's head and emit
    only new's remainder. Uses difflib so a seam the model worded slightly
    differently across the two windows (e.g. 隨口/水口) still collapses. No overlap
    found → append verbatim (never drop real text). Standard LocalAgreement merge.
    """
    committed = (committed or "").strip()
    new = (new or "").strip()
    if not committed:
        return new
    if not new:
        return committed
    cN, _ = _norm_chars(committed)
    nN, nRaw = _norm_chars(new)
    if not cN or not nN:
        return _join(committed, new)

    ctail, nhead = cN[-_SEAM_WIN:], nN[:_SEAM_WIN]
    sm = SequenceMatcher(None, ctail, nhead, autojunk=False)
    # The seam = a substantial matching block ANCHORED at committed's end (the
    # overlap audio re-transcribed at new's head). Require size >= 3 AND that the
    # block reach committed's final chars — this rejects incidental short matches
    # (e.g. "...bar" vs "baz" sharing "ba") that would otherwise cut into a real
    # word. Cut new just past the furthest such block; new's leading chars before
    # it are the overlap re-transcription and are dropped with it.
    cut_norm = 0
    for a, b, size in sm.get_matching_blocks():
        if size >= 3 and a + size >= len(ctail) - 2:
            cut_norm = max(cut_norm, b + size)
    if cut_norm == 0:
        return _join(committed, new)
    raw_cut = nRaw[cut_norm] if cut_norm < len(nRaw) else len(new)
    remainder = new[raw_cut:].lstrip(_LEAD_PUNCT)
    if not remainder:
        return committed
    return _join(committed, remainder)


def _slice_ple(ple, a, b):
    if ple is None:
        return None
    return ple[:, a:b] if ple.ndim >= 2 else ple


class GemmaSession:
    def __init__(self, model, processor, config, instruction=DEFAULT_PROMPT,
                 keepback_tokens=2, max_new=128):
        self.model = model
        self.processor = processor
        self.tok = processor.tokenizer
        self.aid = config["audio_token_id"] if "audio_token_id" in config else 258881
        self.keepback = keepback_tokens
        self.max_new = max_new
        try:
            self.formatted = apply_chat_template(processor, config, instruction, num_audios=1)
        except TypeError:
            self.formatted = apply_chat_template(
                processor, config, instruction, num_images=0, num_audios=1
            )
        self.start()

    # -- lifecycle ----------------------------------------------------------
    def start(self):
        """Open a fresh session.

        `self.audio` / `self.cache` / `self.committed_len` / `self.last_text` hold
        the ACTIVE WINDOW (not the whole recording); `self.committed_text` holds
        the finalised transcript of windows that have already slid out.
        """
        self.cache = cache_mod.make_prompt_cache(self.model.language_model)
        self.committed_len = 0
        self.audio = np.zeros(0, dtype=np.float32)
        self.last_text = ""
        self.committed_text = ""
        return self

    # -- internals ----------------------------------------------------------
    def _build(self):
        """Embeddings for the current full buffer (re-encodes all audio; cheap)."""
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as f:
            sf.write(f.name, self.audio, SR)
            path = f.name
        try:
            inp = prepare_inputs(self.processor, audio=[path], prompts=self.formatted)
        finally:
            os.unlink(path)
        ids = inp["input_ids"]
        if ids.ndim == 1:
            ids = ids[None]
        akw = {
            k: inp[k]
            for k in ("input_features", "input_features_mask")
            if k in inp and inp[k] is not None
        }
        eo = self.model.get_input_embeddings(ids, None, **akw)
        return ids, eo.inputs_embeds, eo.per_layer_inputs

    def _audio_end(self, ids):
        arr = np.asarray(ids[0])
        pos = np.where(arr == self.aid)[0]
        return int(pos[-1] + 1) if len(pos) else self.committed_len

    def _prefill(self, ids, emb, ple, a, b):
        return self.model.language_model(
            inputs=ids[:, a:b],
            inputs_embeds=emb[:, a:b],
            cache=self.cache,
            per_layer_inputs=_slice_ple(ple, a, b),
        )

    def _decode(self, last_logits, cache):
        out, n = [], 0
        y = mx.argmax(last_logits[:, -1, :], axis=-1)
        if int(y.item()) == CHANNEL_ID:
            return None, 0  # reasoning hallucination — not ready to transcribe
        for _ in range(self.max_new):
            yi = int(y.item())
            if yi in STOP_IDS:
                break
            out.append(yi)
            cut = _runaway_cut(out)
            if cut is not None:  # greedy loop — keep one copy, stop decoding
                out = out[:cut]
                break
            yids = y[None]
            eo = self.model.get_input_embeddings(yids, None)
            o = self.model.language_model(
                inputs=yids, inputs_embeds=eo.inputs_embeds,
                cache=cache, per_layer_inputs=eo.per_layer_inputs,
            )
            n += 1
            y = mx.argmax(o.logits[:, -1, :], axis=-1)
        return self.tok.decode(out).strip(), n

    def _decode_window(self):
        """Clean whole-window transcript from a FRESH cache (the authoritative
        decode used on every slide and at finish). Always <= WINDOW_COMMIT_S of
        audio, so it never hits the 30 s cap or the rotating-cache wrap."""
        if len(self.audio) < MIN_AUDIO_SAMPLES or not _has_speech(self.audio):
            return ""
        ids, emb, ple = self._build()
        cache = cache_mod.make_prompt_cache(self.model.language_model)
        o = self.model.language_model(
            inputs=ids, inputs_embeds=emb, cache=cache, per_layer_inputs=ple
        )
        text, _ = self._decode(o.logits, cache)
        return text or ""

    def _preview(self):
        """Full live transcript: committed windows + current-window partial."""
        return _stitch(self.committed_text, self.last_text)

    # -- public API ---------------------------------------------------------
    def feed(self, new_samples) -> str:
        """Append audio, return the full live transcript (committed + partial).

        Stays inside a bounded window: when the active window fills, it is
        committed with a clean fresh-cache decode and slid forward (see _roll).
        """
        self.audio = np.concatenate([self.audio, np.asarray(new_samples, dtype=np.float32)])
        if len(self.audio) < MIN_AUDIO_SAMPLES:
            return self._preview()  # nothing to encode yet

        # Window full → commit it and slide (skip the incremental partial this
        # tick; the fresh-cache _roll decode supersedes it).
        if len(self.audio) >= int(WINDOW_COMMIT_S * SR):
            self._roll()
            return self.committed_text

        # No speech in the window yet → decoding would only hallucinate. Skip
        # the model; the cache is untouched, so the first speechy tick prefills
        # from scratch as usual (committed_len is still 0).
        if not _has_speech(self.audio):
            return self._preview()

        ids, emb, ple = self._build()
        full_len = ids.shape[1]
        audio_end = self._audio_end(ids)

        # Commit settled audio tokens (hold the unstable cut edge).
        commit_end = min(max(self.committed_len, audio_end - self.keepback), full_len)
        if commit_end > self.committed_len:
            self._prefill(ids, emb, ple, self.committed_len, commit_end)
            mx.eval([c.state for c in self.cache])
            self.committed_len = commit_end

        # Partial: prefill remainder (held-back audio + closing), decode, trim back.
        added = 0
        last = None
        if full_len > self.committed_len:
            o = self._prefill(ids, emb, ple, self.committed_len, full_len)
            added += full_len - self.committed_len
            last = o.logits
        if last is None:
            return self._preview()
        text, dec = self._decode(last, self.cache)
        added += dec
        if added > 0 and can_trim_prompt_cache(self.cache):
            trim_prompt_cache(self.cache, added)
        if text is None:  # reasoning hallucination — keep the last good partial
            return self._preview()
        self.last_text = text
        return self._preview()

    def _roll(self):
        """Commit the active window's clean transcript and slide it forward,
        keeping OVERLAP_S of trailing audio as left context for the next window."""
        text = self._decode_window()
        if text:
            self.committed_text = _stitch(self.committed_text, text)
        keep = int(OVERLAP_S * SR)
        self.audio = self.audio[-keep:] if len(self.audio) > keep else self.audio
        self.cache = cache_mod.make_prompt_cache(self.model.language_model)
        self.committed_len = 0
        self.last_text = ""

    def finish(self) -> str:
        """Final authoritative transcript: committed windows + a clean decode of
        the final active window, stitched at the seam.

        Each decode only ever sees <= WINDOW_COMMIT_S of audio (a fresh cache),
        so the final is correct for arbitrarily long recordings — no 30 s
        truncation and no rotating-cache corruption.
        """
        if len(self.audio) < MIN_AUDIO_SAMPLES:
            # Empty/too-short final window — return what's committed (long
            # recording that ended right after a slide), else the last partial.
            return self.committed_text or self.last_text
        text = self._decode_window()
        if not text:  # final window too quiet to transcribe
            return self.committed_text or self.last_text
        return _stitch(self.committed_text, text)
