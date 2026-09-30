import { chromium } from "playwright";

// CI uses Playwright's pinned browser. Developers may opt into a system browser.
// Do not assume a Windows-only cache path or require a Chrome installation.
export function launchBrowser(options = {}) {
  const executablePath = process.env.PLAYWRIGHT_EXECUTABLE_PATH;
  const channel = process.env.PLAYWRIGHT_CHANNEL;
  return chromium.launch({
    ...(executablePath ? { executablePath } : channel ? { channel } : {}),
    ...options,
  });
}
