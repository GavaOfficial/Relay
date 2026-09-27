const { chromium } = require(process.argv[2] || "playwright");
const http = require("node:http");
const fs = require("node:fs");
const path = require("node:path");
const assert = require("node:assert/strict");
const server = http.createServer((req, res) => {
  const name =
    new URL(req.url, "http://localhost").pathname.slice(1) || "index.html";
  if (!["index.html", "app.js", "mock.js", "style.css"].includes(name)) {
    res.writeHead(404).end();
    return;
  }
  res.setHeader(
    "Content-Type",
    name.endsWith(".js")
      ? "text/javascript"
      : name.endsWith(".css")
        ? "text/css"
        : "text/html",
  );
  res.end(fs.readFileSync(path.join(__dirname, "../ui", name)));
});
(async () => {
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  const browser = await chromium.launch({
    executablePath: process.env.RELAY_TEST_BROWSER,
  });
  try {
    const page = await browser.newPage({
      viewport: { width: 480, height: 740 },
    });
    const errors = [];
    page.on("pageerror", (e) => errors.push(e.message));
    await page.goto(`http://127.0.0.1:${server.address().port}/?mock=1`);
    await page.locator("[data-act=login]").click();
    await page.locator("#v-home").waitFor({ state: "visible" });
    await page.screenshot({ path: "relay-home-preview.png", fullPage: true });
    await page.locator("[data-act=gear]").click();
    await page.locator("[data-act=preset][data-id=medium]").click();
    await page.waitForFunction(
      () => document.querySelector("#savemsg").textContent === "Salvato",
    );
    await page.screenshot({
      path: "relay-settings-preview.png",
      fullPage: true,
    });
    await page.locator("[data-act=back]").click();
    await page.locator("[data-act=create]").click();
    await page.setViewportSize({ width: 960, height: 740 });
    await page
      .locator('[data-f=window] option[value="0"]')
      .waitFor({ state: "attached" });
    await page.locator("[data-f=window]").selectOption("0");
    await page.waitForTimeout(1000);
    await page.screenshot({ path: "relay-lobby-preview.png", fullPage: true });
    for (const width of [320, 480, 960]) {
      await page.setViewportSize({ width, height: 740 });
      const overflow = await page.evaluate(() =>
        [...document.querySelectorAll("body *")]
          .filter((e) => e.getBoundingClientRect().right > innerWidth + 1)
          .map((e) => e.tagName + "." + e.className),
      );
      assert(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= innerWidth,
        ),
        `Overflow at ${width}px: ${overflow}`,
      );
    }
    await page.setViewportSize({ width: 960, height: 740 });
    await page.locator("[data-act=start]").click({ timeout: 15000 });
    await page.locator("#elapsed").waitFor({ state: "visible" });
    await page.locator("[data-act=stop]").click();
    await page.locator("[data-act=replay]").waitFor({ state: "visible" });
    await page.locator("[data-act=newmatch]").click();
    await page.locator("#v-home").waitFor({ state: "visible" });
    assert.deepEqual(errors, []);
    console.log(
      "PASS: login, home, settings autosave, lobby, responsive widths, recording, upload, completion; no JS errors. Mock backend only.",
    );
  } finally {
    await browser.close();
  }
})()
  .catch((e) => {
    console.error(e);
    process.exitCode = 1;
  })
  .finally(() => server.close());
