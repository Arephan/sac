// Fake credentials shaped like real ones, so detectors fire. They are built from fragments
// so no token-shaped literal sits in the repository.
const j = (...p: string[]) => p.join("");

export const FAKE = {
  GH_TOKEN: j("gh", "p_", "Fx7Qm2Lk9Vb4Nc8Rt1Yw6Ps3Hd0Gj5Za2Ue4"),
  GH_MCP: j("gh", "p_", "Mq3Wt8Yk1Lz6Xv0Nb5Cr2Hg9Pd4Sf7Ja1Ke3"),
  SLACK_BOT: j("xo", "xb-", "1234567890123", "-", "1234567890123", "-", "AbCdEfGhIjKlMnOpQrStUvWx"),
  SLACK_MCP: j("xo", "xb-", "2234567890123", "-", "2234567890123", "-", "BbCdEfGhIjKlMnOpQrStUvWx"),
  SLACK_JOB: j("xo", "xb-", "3234567890123", "-", "3234567890123", "-", "CbCdEfGhIjKlMnOpQrStUvWx"),
  SLACK_SPARE: j("xo", "xb-", "4234567890123", "-", "4234567890123", "-", "DbCdEfGhIjKlMnOpQrStUvWx"),
  LINEAR: j("Bearer lin", "_api_", "Kq7Wz2Lm9Xv4Bn8Ct1Yp6Rs3Hd0Gf5Ja2Ue4Ti"),
  DEPLOY_KEY: j("dk", "_live_", "Zx9Cv8Bn7Mq6Wt5Yk4Lz3Xv2Nb1"),
};

export const ALL_FAKE_VALUES = Object.values(FAKE);
