// Example custom policy check. Reference it from a policy with `check: max_enabled_loops`.
export default {
  id: 'max_enabled_loops',
  doc: 'At most params.max loops are enabled at once (keeps token spend predictable).',
  run(setup, { max = 10 } = {}) {
    const on = (setup.loops || []).filter((l) => l.enabled).map((l) => l.id);
    return on.length > max
      ? { status: 'fail', message: `${on.length} enabled loops > ${max}`, evidence: on }
      : { status: 'pass', message: `${on.length} enabled loop(s) <= ${max}`, evidence: [] };
  },
};
