#!/usr/bin/env node
/* Browser checks for a rendered record page. What --verify cannot catch: whether the
   provenance layer actually behaves. Both themes, because dark breaks in the one
   combination nobody exercises. Waits on elements, never on a fixed sleep - a flaky
   assertion is worse than none. */
const fs = require('fs');
const path = require('path');
// The driver is not shipped with the plugin. Look for it beside the page first - the
// project's own node_modules, including where pnpm keeps transitive packages - and when
// it is nowhere, say exactly what to do instead of dying on a require.
const chromium = (() => {
  const roots = [path.join(process.cwd(), 'node_modules')];
  const store = path.join(roots[0], '.pnpm');
  if (fs.existsSync(store))
    for (const d of fs.readdirSync(store))
      if (d.startsWith('playwright-core@')) roots.push(path.join(store, d, 'node_modules'));
  for (const spec of ['playwright-core', ...roots.map(r => path.join(r, 'playwright-core'))]) {
    try { return require(spec).chromium; } catch (e) { if (e.code !== 'MODULE_NOT_FOUND') throw e; }
  }
  console.log('verify_page.js needs playwright-core, the driver that opens the page in Chrome; it is not bundled.');
  console.log("Point NODE_PATH at a node_modules that has it - the project's own, if it uses Playwright - or install one beside the page:");
  console.log('  npm i --no-save playwright-core && NODE_PATH="$PWD/node_modules" node verify_page.js record.html');
  process.exit(1);
})();
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
      const hasNow = await p.locator('.tabs button[data-tab=now]').count() > 0;
      if (hasNow) {
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
      if (await p.locator('.tabs button[data-tab=tree]').count()) {
        // The entrance runs once per page load, so the count has to be taken in the
        // same turn as the click that starts it - a round trip in between and it is
        // over before we look.
        const growing = await p.evaluate(() => {
          document.querySelector('.tabs button[data-tab=tree]').click();
          const s = document.querySelector('#panel-tree svg');
          return s && s.getAnimations ? s.getAnimations({ subtree: true }).length : 0;
        });
        chk(`${T} the tree grows in the first time it is opened`, growing > 0);
        chk(`${T} the tree shows the whole record as one shape`,
            await p.locator('#panel-tree svg').isVisible()
            && await p.locator('#panel-tree g[data-id]').count() > 0);
        const tn = p.locator('#panel-tree g[data-id]').last();
        await tn.scrollIntoViewIfNeeded(); await p.mouse.move(2, 2); await tn.click();
        chk(`${T} a tree node opens the same card`, await seen(p, '.pop'));
        if (await p.locator('.pop .tfoc').count()) {
          await p.locator('.pop .tfoc').click();
          chk(`${T} focus prunes the tree to one node's world`,
              await p.locator('#panel-tree .tn.hid').count() > 0
              && await p.locator('#focchip').isVisible());
          await p.locator('#focchip').click();
          chk(`${T} and the chip brings the whole tree back`,
              await p.locator('#panel-tree .tn.hid').count() === 0
              && await p.locator('#focchip').count() === 0);
        } else {
          await p.keyboard.press('Escape');
        }

        // Pulling a node. It follows part of the way and then plainly refuses, and
        // nothing it disturbed stays disturbed. The node with the most company is
        // picked on purpose: that is the one whose neighbours have to give way, so
        // the recovery being asserted is a real one wherever the record is crowded.
        await p.keyboard.press('Escape');
        const busiest = await p.evaluate(() => {
          const s = document.querySelector('#panel-tree svg');
          const rad = s.viewBox.baseVal.width / 8 * 0.62;
          const ns = [...s.querySelectorAll('.tn[data-id]')].map(g => {
            const c = g.querySelector('circle');
            return { id: g.getAttribute('data-id'),
                     x: +c.getAttribute('cx'), y: +c.getAttribute('cy') };
          });
          let best = { id: null, near: -1, mate: null };
          for (const n of ns) {
            const company = ns.filter(m => m !== n && Math.hypot(m.x - n.x, m.y - n.y) < rad);
            if (company.length > best.near)
              best = { id: n.id, near: company.length, mate: company[0] ? company[0].id : null };
          }
          return best;
        });
        // Aim at the circle, not the group: a labelled node's group box spans the
        // dot and the text under it, and its centre is the gap between them.
        const dn = p.locator(`#panel-tree g[data-id="${busiest.id}"] circle:not(.halo)`).first();
        await dn.scrollIntoViewIfNeeded();
        const b0 = await dn.boundingBox();
        const PULL = 300;
        // limb geometry is read as numbers, not as the `d` string, so a control point
        // that prints as -0 in one pass and 0 in the other cannot fail the comparison
        const limbGeom = () => p.evaluate(() =>
          [...document.querySelectorAll('#panel-tree path[data-lt]')]
            .map(x => (x.getAttribute('d').match(/-?[\d.]+/g) || []).map(Number)));
        const same = (a, c) => a.length === c.length && a.every((r, i) =>
          r.length === c[i].length && r.every((v, j) => Math.abs(v - c[i][j]) <= 0.05));
        const geom0 = await limbGeom();
        // grabs wherever the node is now, which during a spring is not where it lives
        const pull = async (loc, dist) => {
          const from = await loc.boundingBox();
          const px = from.x + from.width / 2, py = from.y + from.height / 2;
          const dir = px < 550 ? 1 : -1;             // pull inward, never off the viewport
          await p.mouse.move(px, py);
          await p.mouse.down();
          for (let i = 1; i <= 10; i++) await p.mouse.move(px + dir * dist * i / 10, py);
          const to = await loc.boundingBox();
          await p.mouse.up();
          return Math.hypot(to.x - from.x, to.y - from.y);
        };
        const went = await pull(dn, PULL);
        chk(`${T} a pulled node resists - ${Math.round(went)}px of a ${PULL}px pull, `
            + `${busiest.near} neighbours giving way`, went > 20 && went < PULL * 0.6);
        // Now grab a neighbour that is still recovering from that pull. Its limbs are
        // mid-flight, and a fresh grab that read them as where they live would put the
        // tree back to a lie - one that outlasts the gesture.
        if (busiest.mate)
          await pull(p.locator(
            `#panel-tree g[data-id="${busiest.mate}"] circle:not(.halo)`).first(), PULL / 2);
        const adrift = () => p.evaluate(() =>
          [...document.querySelectorAll('#panel-tree .tn[data-id]')]
            .filter(g => g.style.transform).length);
        // Wait for the tree to go quiet, not merely for it to look right once. Two
        // springs racing pass through the true positions on their way to a wrong
        // resting place, so only stillness is worth reading.
        let off = PULL, left = 1, geom = null, prev = null, still = 0;
        for (let i = 0; i < 40 && still < 3; i++) {
          await p.waitForTimeout(50);
          geom = await limbGeom();
          left = await adrift();
          const b2 = await dn.boundingBox();
          off = Math.hypot(b2.x - b0.x, b2.y - b0.y);
          still = (prev && same(geom, prev) && !left && off <= 1) ? still + 1 : 0;
          prev = geom;
        }
        chk(`${T} and every node and limb it moved is back where the record put it`,
            off <= 1 && left === 0 && same(geom, geom0));
      }

      if (errs.length) console.log('      ' + errs.join(' | '));
    } catch (e) {
      chk(`${T} harness error: ${e.message}`, false);
    } finally {
      if (ctx) await ctx.close();
    }
  }

  // Reduced motion is a request, not a preference to be weighed against a nice
  // effect: the tree is simply there, with nothing to sit through.
  {
    const T = '[reduced motion]';
    let ctx;
    try {
      ctx = await b.newContext({ viewport: { width: 1100, height: 900 }, reducedMotion: 'reduce' });
      const p = await ctx.newPage();
      const errs = []; p.on('pageerror', e => errs.push(String(e)));
      await p.goto('file://' + require('path').resolve(FILE) + '#tree');
      await p.locator('#panel-tree g[data-id]').first().waitFor();
      const st = await p.evaluate(() => {
        const s = document.querySelector('#panel-tree svg');
        return { nodes: s.querySelectorAll('.tn[data-id]').length,
                 anims: s.getAnimations ? s.getAnimations({ subtree: true }).length : -1,
                 part: [...s.querySelectorAll('path')]
                         .filter(x => x.style.strokeDashoffset !== '').length };
      });
      chk(`${T} no page errors`, errs.length === 0);
      chk(`${T} the tree is whole the moment it is opened, nothing to sit through`,
          st.nodes > 0 && st.anims === 0 && st.part === 0);
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
