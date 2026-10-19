export interface Target {
  home: string; // absolute home directory
  claudeDir: string; // ~/.claude, or the --config-dir
  claudeJson: string; // ~/.claude.json, or <config-dir>/.claude.json
  configDir: string | null;
  platform: "darwin" | "linux";
}

/** A file under the Claude folder. `path` is relative to it, with "/" separators. */
export interface SavedFile {
  path: string;
  data: Buffer;
  mode: number;
}

export type McpServer = Record<string, unknown>;

export interface Marketplace {
  name: string;
  source: string;
  kind: "github" | "git" | "directory" | "other";
}

export interface PluginEntry {
  id: string;
  enabled: boolean;
  settings: Record<string, string>;
}

export interface JobDef {
  label: string;
  kind: "launchd" | "cron";
  program: string[];
  /** launchd keys (StartCalendarInterval, StartInterval, KeepAlive, RunAtLoad, ...) or { cron: "m h dom mon dow" } */
  schedule: Record<string, unknown>;
  workingDirectory?: string;
  env: Record<string, string>;
  stdout?: string;
  stderr?: string;
  dependencies: string[];
  /** plist keys sac does not model, kept verbatim */
  extra?: Record<string, unknown>;
}

export interface Setup {
  name: string;
  captured?: { at: string; claude: string | null; os: string };
  extends: string[];
  files: SavedFile[];
  mcp: Record<string, McpServer>;
  marketplaces: Marketplace[];
  plugins: PluginEntry[];
  jobs: JobDef[];
}

export interface SecretRef {
  name: string;
  where: string;
}

export interface Warning {
  kind: "dependency" | "secret-in-file" | "skipped" | "clash" | "login-needed" | "other";
  message: string;
}
