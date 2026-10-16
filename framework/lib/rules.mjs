// Turning free-text instructions into structured rules: severity, scope, date, reason.
// The keyword table is generic; an instance can extend it via
// import.options.<importer>.scope_keywords: { scope: [words...] }.

export const DEFAULT_SCOPE_KEYWORDS = {
  money: ['spend', 'purchase', 'pay', 'payment', 'subscription', 'subscribe', 'billing', 'buy', 'refund', 'price', 'card'],
  email: ['email', 'gmail', 'inbox', 'mail'],
  chat: ['slack', 'discord', 'teams', 'whatsapp', 'telegram', 'reply', 'thread'],
  social: ['reddit', 'twitter', ' x ', 'tiktok', 'instagram', 'threads', 'youtube', 'shorts', 'reels', 'post', 'tweet', 'linkedin'],
  press: ['press', 'journalist', 'reporter', 'pitch', 'media outlet'],
  apps: ['app store', 'testflight', 'play store', 'review', 'build', 'release', 'listing', 'aso', 'paywall'],
  jobs: ['job', 'resume', 'recruiter', 'interview', 'salary', 'apply'],
  contracts: ['bid', 'tender', 'contract', 'rfp', 'procurement'],
  calendar: ['calendar', 'appointment', 'booking', 'schedule', 'custody', 'event'],
  browser: ['chrome', 'browser', 'playwright', 'cdp'],
  privacy: ['phone', 'address', 'pii', 'customer name', 'identity', 'separation'],
  code: ['git', 'repo', 'commit', 'deploy', 'test', 'simulator', 'backend', 'component'],
  loops: ['loop', 'launchd', 'cron', 'daemon', 'every run'],
  design: ['design', 'ui', 'icon', 'mockup', 'screenshot'],
};

export function classifyScope(text, extra = {}) {
  const t = ' ' + String(text).toLowerCase() + ' ';
  const table = { ...DEFAULT_SCOPE_KEYWORDS };
  for (const [k, words] of Object.entries(extra || {})) table[k] = [...(table[k] || []), ...words];
  let best = 'global', bestN = 0;
  for (const [scope, words] of Object.entries(table)) {
    const n = words.reduce((acc, w) => acc + (t.includes(w.toLowerCase()) ? 1 : 0), 0);
    if (n > bestN) { best = scope; bestN = n; }
  }
  return best;
}

// A rule is hard only when its author marked it so (uppercase HARD); everything else is standing.
export function severityOf(text) {
  return /\bHARD\b/.test(String(text)) ? 'hard' : 'standing';
}

export function firstDate(...texts) {
  for (const t of texts) {
    const m = String(t || '').match(/\b(20\d\d)-(\d\d)-(\d\d)\b/);
    if (m) return `${m[1]}-${m[2]}-${m[3]}`;
  }
  return null;
}

export function oneLine(s, max = 280) {
  const t = String(s || '').replace(/\s+/g, ' ').trim();
  return t.length > max ? t.slice(0, max - 1) + '…' : t;
}
