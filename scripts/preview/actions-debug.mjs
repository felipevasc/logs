export default async (page) => {
  await page.evaluate(() => document.querySelector("#btn-right-case").click());
  await page.waitForTimeout(1200);
  const info = await page.evaluate(() => {
    const main = document.querySelector("#view-analysis");
    const panel = main.querySelector(".group-panel");
    const list = document.querySelector("#analysis-list");
    const r = (el) => { const b = el.getBoundingClientRect(); return { x: Math.round(b.x), y: Math.round(b.y), w: Math.round(b.width), h: Math.round(b.height) }; };
    return {
      mainHidden: main.hidden,
      mainRect: r(main),
      sideRect: r(main.querySelector(".view-side")),
      panelRect: r(panel),
      panelDisplay: getComputedStyle(panel).display,
      panelChildren: panel.children.length,
      listChildren: list.children.length,
      listHTML: list.innerHTML.slice(0, 200),
      bodyClass: document.body.className,
    };
  });
  console.log(JSON.stringify(info, null, 2));
};
