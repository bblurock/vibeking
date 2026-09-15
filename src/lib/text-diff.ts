// Word-level diff between two strings for the chip bar's "done" state.
//
// Tokenises preserving whitespace + punctuation as separate tokens, runs
// a longest-common-subsequence walk, and returns a list of operations
// that the renderer can colour individually. Lightweight — no deps —
// because the chip bar only ever diffs ~200-char dictation transcripts.

export type DiffOp =
  | { kind: "equal"; text: string }
  | { kind: "removed"; text: string }
  | { kind: "added"; text: string };

// Tokenise into runs of whitespace, runs of punctuation, and runs of
// "word characters" (anything else). CJK glyphs each end up as their own
// "word" token, which is exactly the granularity we want for diffing
// 中文 + English mixed transcripts.
function tokenize(s: string): string[] {
  if (!s) return [];
  const out: string[] = [];
  // Word = Unicode letter/number block. Punctuation/whitespace each
  // form their own token. CJK letters split into per-char tokens via
  // the `Script_Extensions` properties.
  const re =
    /\p{Script=Han}|\p{Script=Hiragana}|\p{Script=Katakana}|\p{Script=Hangul}|[\p{L}\p{N}_]+|\s+|[^\p{L}\p{N}\s]/gu;
  for (const m of s.matchAll(re)) out.push(m[0]);
  return out;
}

function lcsMatrix(a: string[], b: string[]): number[][] {
  const m = a.length;
  const n = b.length;
  const dp: number[][] = Array.from({ length: m + 1 }, () =>
    new Array<number>(n + 1).fill(0),
  );
  for (let i = 1; i <= m; i++) {
    for (let j = 1; j <= n; j++) {
      dp[i][j] =
        a[i - 1] === b[j - 1]
          ? dp[i - 1][j - 1] + 1
          : Math.max(dp[i - 1][j], dp[i][j - 1]);
    }
  }
  return dp;
}

/**
 * Word-level diff. Equal tokens that differ only in surrounding
 * whitespace get merged into a single equal run so the rendered output
 * doesn't fragment into hundreds of micro-spans.
 */
export function diffWords(raw: string, polished: string): DiffOp[] {
  if (raw === polished) return [{ kind: "equal", text: polished }];
  if (!raw) return [{ kind: "added", text: polished }];
  if (!polished) return [{ kind: "removed", text: raw }];

  const a = tokenize(raw);
  const b = tokenize(polished);
  const dp = lcsMatrix(a, b);

  // Walk back through the matrix to emit operations.
  const reversed: DiffOp[] = [];
  let i = a.length;
  let j = b.length;
  const push = (op: DiffOp) => {
    const last = reversed[reversed.length - 1];
    if (last && last.kind === op.kind) {
      last.text = op.text + last.text;
    } else {
      reversed.push(op);
    }
  };
  while (i > 0 && j > 0) {
    if (a[i - 1] === b[j - 1]) {
      push({ kind: "equal", text: a[i - 1] });
      i--;
      j--;
    } else if (dp[i - 1][j] >= dp[i][j - 1]) {
      push({ kind: "removed", text: a[i - 1] });
      i--;
    } else {
      push({ kind: "added", text: b[j - 1] });
      j--;
    }
  }
  while (i > 0) {
    push({ kind: "removed", text: a[--i] });
  }
  while (j > 0) {
    push({ kind: "added", text: b[--j] });
  }
  return reversed.reverse();
}
