import { lintSource } from "@secretlint/core";
import { creator as presetRecommend } from "@secretlint/secretlint-rule-preset-recommend";
import type { SecretLintCoreConfig } from "@secretlint/types";
import type { SecretRef } from "./types.js";

/** Keys whose values are credentials. */
export const SECRET_KEY =
  /token|secret|passw(or)?d|credential|api[_-]?key|private[_-]?key|access[_-]?key|authorization|(^|[_-])auth($|[_-])|(^|[_-])key$|(^|[_-])pat$/i;

/** A credential flag in an argument list, such as --token or --api-key. */
const SECRET_FLAG = /^--?[A-Za-z0-9_-]+$/;

const PLACEHOLDER = /\{\{secret:([A-Za-z_][A-Za-z0-9_]*)\}\}/g;
export const ph = (name: string) => `{{secret:${name}}}`;

const config: SecretLintCoreConfig = { rules: [{ id: "@secretlint/secretlint-rule-preset-recommend", rule: presetRecommend }] };

async function lint(content: string) {
  const r = await lintSource({
    source: { filePath: "/sac-value.txt", content, ext: ".txt", contentType: "text" },
    options: { config, maskSecrets: true, noPhysicFilePath: true },
  });
  return r.messages;
}

/** Ranges of detected credentials inside a string, in order, without overlaps. */
export async function findSecretSpans(value: string): Promise<Array<[number, number]>> {
  if (value.length < 8) return [];
  const spans = (await lint(value)).map((m) => m.range as [number, number]).sort((a, b) => a[0] - b[0]);
  const out: Array<[number, number]> = [];
  for (const s of spans) if (!out.length || s[0] >= out[out.length - 1][1]) out.push(s);
  return out;
}

export async function looksSecret(value: string): Promise<boolean> {
  return (await findSecretSpans(value)).length > 0;
}

/** Lines of a text file that hold a credential, with the detecting rule. Never the value. */
export async function scanText(content: string): Promise<Array<{ line: number; rule: string }>> {
  return (await lint(content)).map((m) => ({
    line: m.loc.start.line,
    rule: String(m.ruleId).replace(/^@secretlint\/secretlint-rule-/, ""),
  }));
}

const clean = (s: string) => s.replace(/[^A-Za-z0-9]+/g, "_").replace(/^_+|_+$/g, "").toUpperCase();

/** Gives each secret value a stable placeholder name and remembers where it was found. */
export class SecretVault {
  private byValue = new Map<string, string>();
  private byName = new Map<string, string>();
  private where = new Map<string, string>();

  name(path: string[], value: string): string {
    const known = this.byValue.get(value);
    if (known) return known;
    const last = path[path.length - 1] ?? "";
    const base =
      path[path.length - 2] === "env"
        ? /^[A-Za-z_][A-Za-z0-9_]*$/.test(last) ? last : clean(last)
        : clean(path.slice(1).filter((p) => p !== "headers").map((p) => p.replace(/@.*$/, "")).join("_"));
    const root = base || "SECRET";
    let name = root;
    for (let i = 2; this.byName.has(name); i++) name = `${root}_${i}`;
    this.byName.set(name, value);
    this.byValue.set(value, name);
    this.where.set(name, path.join("."));
    return name;
  }

  refs(): SecretRef[] {
    return [...this.where.entries()].map(([name, where]) => ({ name, where }));
  }

  values(): Record<string, string> {
    return Object.fromEntries(this.byName);
  }
}

export interface RedactOptions {
  /** Keys whose values are always secret (a plugin's sensitive options). */
  sensitiveKeys?: Set<string>;
  /** Where a credential-looking key makes its value secret: under env/headers only, or anywhere. */
  byKey?: "env" | "all";
}

/** Replaces credentials with {{secret:NAME}} placeholders, recording each in the vault. */
export async function redact<T>(v: T, base: string[], vault: SecretVault, opts: RedactOptions = {}): Promise<T> {
  const byKey = opts.byKey ?? "env";
  const walk = async (x: unknown, path: string[], whole: boolean): Promise<unknown> => {
    if (typeof x === "string") {
      if (x === "" || new RegExp(`^${PLACEHOLDER.source}$`).test(x)) return x;
      const key = path[path.length - 1] ?? "";
      const parent = path[path.length - 2];
      const keyHit = SECRET_KEY.test(key) && (byKey === "all" || parent === "env" || parent === "headers");
      if (whole || keyHit || opts.sensitiveKeys?.has(key)) return ph(vault.name(path, x));
      const spans = await findSecretSpans(x);
      let out = x;
      for (const [a, b] of spans.reverse()) out = out.slice(0, a) + ph(vault.name(path, x.slice(a, b))) + out.slice(b);
      return out;
    }
    if (Array.isArray(x)) {
      const out: unknown[] = [];
      for (let i = 0; i < x.length; i++) {
        const prev = x[i - 1];
        const flagged = typeof prev === "string" && SECRET_FLAG.test(prev) && SECRET_KEY.test(prev.replace(/^-+/, ""));
        out.push(await walk(x[i], [...path, String(i)], flagged));
      }
      return out;
    }
    if (x && typeof x === "object") {
      const out: Record<string, unknown> = {};
      for (const [k, y] of Object.entries(x as Record<string, unknown>)) out[k] = await walk(y, [...path, k], false);
      return out;
    }
    return x;
  };
  return (await walk(v, base, false)) as T;
}

/** Placeholder names used anywhere in a value, sorted and unique. */
export function placeholdersIn(v: unknown): string[] {
  const found = new Set<string>();
  const walk = (x: unknown) => {
    if (typeof x === "string") for (const m of x.matchAll(PLACEHOLDER)) found.add(m[1]);
    else if (Array.isArray(x)) x.forEach(walk);
    else if (x && typeof x === "object" && !Buffer.isBuffer(x)) Object.values(x).forEach(walk);
  };
  walk(v);
  return [...found].sort();
}

export function fillText(s: string, values: Record<string, string>, missing?: Set<string>): string {
  return s.replace(PLACEHOLDER, (m, n: string) => {
    if (Object.hasOwn(values, n)) return values[n];
    missing?.add(n);
    return m;
  });
}

export function fillPlaceholders<T>(v: T, values: Record<string, string>): { value: T; missing: string[] } {
  const missing = new Set<string>();
  const walk = (x: unknown): unknown => {
    if (typeof x === "string") return fillText(x, values, missing);
    if (Array.isArray(x)) return x.map(walk);
    if (x && typeof x === "object" && !Buffer.isBuffer(x)) return Object.fromEntries(Object.entries(x).map(([k, y]) => [k, walk(y)]));
    return x;
  };
  return { value: walk(v) as T, missing: [...missing].sort() };
}

const escapeRe = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** Values for the placeholders in `template`, read from the same places in `actual`. */
export function extractSecrets(template: unknown, actual: unknown): Record<string, string> {
  const found: Record<string, string> = {};
  const walk = (t: unknown, a: unknown) => {
    if (typeof t === "string") {
      if (typeof a !== "string") return;
      const names: string[] = [];
      const parts = t.split(PLACEHOLDER);
      let re = "^";
      parts.forEach((p, i) => {
        if (i % 2 === 1) {
          names.push(p);
          re += "([\\s\\S]*?)";
        } else re += escapeRe(p);
      });
      if (!names.length) return;
      const m = new RegExp(`${re}$`).exec(a);
      if (m) names.forEach((n, i) => { found[n] ??= m[i + 1]; });
    } else if (Array.isArray(t)) {
      if (Array.isArray(a)) t.forEach((x, i) => walk(x, a[i]));
    } else if (t && typeof t === "object") {
      if (a && typeof a === "object" && !Array.isArray(a)) for (const [k, x] of Object.entries(t)) walk(x, (a as Record<string, unknown>)[k]);
    }
  };
  walk(template, actual);
  return found;
}

/** Hides known secret values in text meant for people (CLI errors, logs). */
export function scrub(text: string, values: Iterable<string>): string {
  let out = text;
  for (const v of values) if (v && v.length >= 4) out = out.split(v).join("***");
  return out;
}
