// Preview-only instrumentation. WAAPI pause()/play() detach CSSAnimation playback
// from later animation-play-state changes, including the product's pause button.
// Install this self-contained function before navigation with page.addInitScript.
export function installMotionSampling() {
  const property = 'animation-play-state';
  const frames = () => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  window.__waitingMotionSampling = {
    async begin(input) {
      const animations = [...new Set(input)];
      const times = animations.map(animation => ({ animation, time: animation.currentTime }));
      const targets = [...new Set(animations.map(animation => animation.effect.target))];
      const styles = targets.map(target => ({ target,
        value: target.style.getPropertyValue(property), priority: target.style.getPropertyPriority(property) }));
      const flush = () => {
        for (const target of targets) getComputedStyle(target).getPropertyValue(property);
      };
      const pauseCss = () => {
        for (const target of targets) target.style.setProperty(property, 'paused', 'important');
        flush();
      };
      const restoreCss = () => {
        for (const { target, value, priority } of styles) {
          if (value) target.style.setProperty(property, value, priority);
          else target.style.removeProperty(property);
        }
        flush();
      };
      let restored = false;
      const sampling = {
        async seek(time) {
          for (const animation of animations) animation.currentTime = time;
          await frames();
        },
        // Resume only the original CSS policy for full-speed video capture. A
        // work track paused by the product must never be forced to run here.
        async release() { restoreCss(); await frames(); },
        async restore() {
          if (restored) return;
          try {
            pauseCss();
            for (const { animation, time } of times) animation.currentTime = time;
          } finally {
            restoreCss();
            restored = true;
          }
          await frames();
        },
      };
      try {
        pauseCss();
        await frames();
        return sampling;
      } catch (error) {
        await sampling.restore();
        throw error;
      }
    },
  };
}
