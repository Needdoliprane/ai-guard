import { createRequire } from 'node:module';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import assert from 'node:assert/strict';
const require = createRequire('/Users/louis/.npm/_npx/9833c18b2d85bc59/node_modules/');
const { chromium } = require('playwright');
const origin = 'http://127.0.0.1:48400';
const output = new URL('./captures/', import.meta.url);
let token = '', browser;
const captures = [], initialLogin = [];
async function capture(page, name) {
  await page.evaluate(() => scrollTo(0,0));
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
  assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth), false, 'No global overflow');
  await page.screenshot({path:new URL(name,output).pathname});
  captures.push(name);
}
try {
  await mkdir(output,{recursive:true});
  token=(await readFile('/tmp/ai-guard-control-qa.mF6i7C/config/secrets/viewer','utf8')).trim();
  browser=await chromium.launch({executablePath:'/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',headless:true});
  const served=await fetch(origin+'/favicon.svg?v=ovpt-20261030',{redirect:'error'});
  assert.equal(served.status,200);
  assert.equal(await served.text(),await readFile('/Users/louis/Documents/ai-guard/front/dist/favicon.svg','utf8'));
  for (const theme of ['light','dark']) {
    const context=await browser.newContext({viewport:{width:320,height:568},colorScheme:theme,reducedMotion:'reduce',serviceWorkers:'block'});
    await context.route('**/*',r=>new URL(r.request().url()).origin===origin?r.continue():r.abort());
    const page=await context.newPage();
    await page.goto(origin,{waitUntil:'domcontentloaded'});
    await page.getByRole('button',{name:'Ouvrir la supervision',exact:true}).waitFor();
    const geometry=await page.evaluate(()=>({theme:document.documentElement.dataset.theme,scrollY,action:document.querySelector('.login-submit').getBoundingClientRect().bottom,brand:document.querySelector('.brand').getBoundingClientRect().top,width:innerWidth,height:innerHeight}));
    assert.equal(geometry.theme,theme);assert.equal(geometry.scrollY,0);assert.ok(geometry.brand>=0);assert.ok(geometry.action<=geometry.height);
    initialLogin.push(geometry);
    await capture(page,'login-320-'+theme+'-real.png');
    await page.setViewportSize({width:1440,height:1050});
    await page.locator('#viewer-token').fill(token);
    await page.getByRole('button',{name:'Ouvrir la supervision',exact:true}).click();
    await page.getByRole('heading',{level:1,name:'Vue d’ensemble',exact:true}).waitFor();
    await page.locator('.table-scroll tbody tr').first().waitFor();
    assert.equal((await page.content()).includes(token),false);
    await capture(page,'overview-desktop-'+theme+'-real.png');
    await page.locator('.nav-link[data-view="workers"]').click();
    await page.getByRole('heading',{level:1,name:'Workers',exact:true}).waitFor();
    await page.locator('.control-panel tbody tr').first().waitFor();
    await capture(page,'workers-desktop-'+theme+'-real.png');
    await page.setViewportSize({width:390,height:844});
    await capture(page,'workers-mobile-'+theme+'-real.png');
    await page.setViewportSize({width:844,height:390});
    await capture(page,'workers-landscape-'+theme+'-real.png');
    await context.close();
  }
  await writeFile(new URL('./live-capture-result.json',import.meta.url),JSON.stringify({status:'PASS',origin,source:'real QA gateway/dashboard, existing durable audit and worker records',favicon:'HTTP200 and exact build bytes',initialLogin,captures},null,2));
  console.log('PASS: served favicon, two real themes, initial login geometry and 10 real screenshots');
} catch(error) {
  let message=String(error?.stack??error);if(token)message=message.replaceAll(token,'[MASKED]');
  console.error(message);process.exitCode=1;
} finally {await browser?.close();}

