const DEFAULT_SEEK_SETTLE_TOLERANCE_SECONDS = 0.5;

function normalizeSeconds(seconds) {
  return Number.isFinite(seconds) ? Math.max(0, seconds) : 0;
}

export function createSeekState({
  settleToleranceSeconds = DEFAULT_SEEK_SETTLE_TOLERANCE_SECONDS,
} = {}) {
  let requestedSeconds = null;
  let committed = false;
  const toleranceSeconds = Math.max(0, settleToleranceSeconds);

  return {
    update(seconds) {
      requestedSeconds = normalizeSeconds(seconds);
      committed = false;
      return requestedSeconds;
    },

    commit(fallbackSeconds = 0) {
      if (requestedSeconds === null) {
        requestedSeconds = normalizeSeconds(fallbackSeconds);
      }
      committed = true;
      return requestedSeconds;
    },

    clear() {
      requestedSeconds = null;
      committed = false;
    },

    getDisplayTime(actualSeconds) {
      return requestedSeconds ?? normalizeSeconds(actualSeconds);
    },

    settle(actualSeconds) {
      if (requestedSeconds === null || !committed) return false;

      const normalizedActualSeconds = normalizeSeconds(actualSeconds);
      if (Math.abs(normalizedActualSeconds - requestedSeconds) > toleranceSeconds) {
        return false;
      }

      requestedSeconds = null;
      committed = false;
      return true;
    },

    isActive() {
      return requestedSeconds !== null;
    },
  };
}
