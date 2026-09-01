/* Browser checks for a rendered record page. What --verify cannot catch: whether the
   provenance layer actually behaves. Both themes, because dark breaks in the one
   combination nobody exercises. Waits on elements, never on a fixed sleep - a flaky
   assertion is worse than none. */
const { chromium } = require('playwright-core');
const fs = require('fs');
const FILE = process.argv[2] || 'record.html';
const CHROME = process.env.CHROME || ['/opt/pw-browsers/chromium-1194/chrome-linux/chrome',
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome'].find(fs.existsSync);
if (!CHROME) { console.log('no Chrome/Chromium found - set CHROME to a browser binary'); process.exit(1); }

(async () => {
  const b = await chromium.launch({ executablePath: CHROME });
  let pass = 0, fail = 0;
  const chk = (n, c) => { console.log((c ? '  ok  ' : '  X   ') + n); c ? pass++ : fail++; };
  const seen = async (p, sel, ms = 4000) =>
    p.locator(sel).first().waitFor({ state: 'attached', timeout: ms }).then(() => true, () => false);
  const txt = async (p, sel = '.pop', ms = 3000) =>
    p.locator(sel).first().innerText({ timeout: ms }).catch(() => '');
  const gone = async (p, sel, ms = 4000) =>
    p.locator(sel).first().waitFor({ state: 'detached', timeout: ms }).then(() => true, () => false);

  for (const theme of ['light', 'dark']) {
    const T = `[${theme}]`;
    let ctx;
    try {
      ctx = await b.newContext({ viewport: { width: 1100, height: 900 }, colorScheme: theme });
      const p = await ctx.newPage();
      const errs = []; p.on('pageerror', e => errs.push(String(e)));
      await p.goto('file://' + require('path').resolve(FILE));
      await p.locator('[data-id]').first().waitFor();

      chk(`${T} no page errors`, errs.length === 0);
      chk(`${T} no horizontal overflow`, await p.evaluate(
        () => document.documentElement.scrollWidth <= window.innerWidth + 1));

      // Scroll, plant the cursor, then move onto it. A move dispatched in the same frame
      // as a scroll produces no mouseover in a driven browser - that is the harness, not
      // the page, and it is what made this check flaky before.
      // whatever tab you land on, hover the first value it shows - a table cell if the
      // visible section has one, else whatever data-id it does have (alerts/timeline/cards)
      let el = p.locator('section:not([hidden]) td [data-id]').first();
      if (!(await el.count())) el = p.locator('section:not([hidden]) [data-id]').first();
      await el.scrollIntoViewIfNeeded();
      await p.mouse.move(2, 2);
      await el.hover();
      chk(`${T} hover opens the source`, await seen(p, '.pop'));
      // the fallback above may land on a judgment instead of an entry - its popover has
      // no from/value, but every judgment popover names what it rests on unconditionally
      chk(`${T} it names where the value came from`, /from|rule|value|file|url|rests on/i.test(await txt(p)));

      const pb = await p.locator('.pop').boundingBox();
      await p.mouse.move(pb.x + pb.width / 2, pb.y + pb.height / 2);
      await p.waitForTimeout(700);
      chk(`${T} the card survives moving onto it`, await p.locator('.pop').count() === 1);
      await p.mouse.move(4, 4);
      chk(`${T} and closes on leaving`, await gone(p, '.pop'));

      const j = p.locator('.card [data-id]').first();
      if (await j.count()) {
        await j.scrollIntoViewIfNeeded(); await j.click();
        chk(`${T} a judgment card opens`, await seen(p, '.pop'));
        chk(`${T} it shows the conclusion and what it rests on`,
            /concludes|rests on/i.test(await txt(p)));
        const d = p.locator('.pop .dep').first();
        if (await d.count()) {
          const key = (await d.innerText()).trim();
          await d.click();
          chk(`${T} clicking a dependency walks to it (${key})`,
              await seen(p, '.pop .back') && (await txt(p)).includes(key));
          await p.locator('.pop .back').click();
          chk(`${T} back returns to the judgment`, /rests on/i.test(await txt(p)));
        }
      }
      if (await p.locator('.tabs').count()) {
        chk(`${T} the arrangement is the tab you land on`,
            await p.locator('.tabs button[data-tab=now][aria-selected=true]').count() === 1
            && await p.locator('#panel-record').isHidden());
        await p.locator('.tabs button[data-tab=record]').click();
        chk(`${T} switching shows the record and hides the arrangement`,
            await p.locator('#panel-record').isVisible() && await p.locator('#panel-now').isHidden());
        const r = p.locator('#panel-record td [data-id]').first();
        await r.scrollIntoViewIfNeeded(); await p.mouse.move(2, 2); await r.click();
        chk(`${T} one provenance layer, working on both tabs`, await seen(p, '.pop'));
        await p.locator('.tabs button[data-tab=now]').click();
        chk(`${T} switching tabs closes the open card`, await gone(p, '.pop'));
        chk(`${T} and the arrangement is back`, await p.locator('#panel-now').isVisible());
        await p.goto('file://' + require('path').resolve(FILE) + '#record');
        chk(`${T} a deep link opens the tab it names`,
            await p.locator('#panel-record td [data-id]').first()
                   .waitFor({ timeout: 4000 }).then(() => true, () => false)
            && await p.locator('#panel-now').isHidden());
      }

      if (errs.length) console.log('      ' + errs.join(' | '));
    } catch (e) {
      chk(`${T} harness error: ${e.message}`, false);
    } finally {
      if (ctx) await ctx.close();
    }
  }
  console.log(`\n${pass} passed, ${fail} failed`);
  await b.close();
  process.exit(fail ? 1 : 0);
})();
