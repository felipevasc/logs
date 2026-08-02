export default async (page) => {
  const info = await page.evaluate(() => {
    const sb = document.querySelector(".filters-block .search-box");
    const side = document.querySelector(".side");
    const block = document.querySelector(".filters-block");
    const r = (el) => { const b = el.getBoundingClientRect(); return { x: b.x, right: b.right, w: b.width }; };
    const cs = getComputedStyle(sb);
    return {
      side: r(side), block: r(block), search: r(sb),
      computed: {
        width: cs.width, minWidth: cs.minWidth, flex: cs.flex,
        padding: cs.padding, boxSizing: cs.boxSizing, marginLeft: cs.marginLeft,
      },
      sideScrollW: side.scrollWidth, sideClientW: side.clientWidth,
    };
  });
  console.log(JSON.stringify(info, null, 2));
};
