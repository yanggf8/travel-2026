import type { Env } from './turso';
import { getPlan, getDashboardPlan, getBookings, listPlans, updateDayField, updateSessionField } from './turso';
import { renderDashboard, renderError, renderPlanIndex } from './render';

async function timingSafeEqual(a: string, b: string): Promise<boolean> {
  const encoder = new TextEncoder();
  const aBuf = encoder.encode(a);
  const bBuf = encoder.encode(b);
  if (aBuf.byteLength !== bBuf.byteLength) {
    // Compare against self to keep constant time, then return false
    crypto.subtle.timingSafeEqual(aBuf, aBuf);
    return false;
  }
  return crypto.subtle.timingSafeEqual(aBuf, bBuf);
}

interface EditRequest {
  token: string;
  table: 'days' | 'timesofday';
  plan_id: string;
  destination: string;
  day_number: number;
  session_type?: string;
  field: string;
  value: string;
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    // ZH is default; switch to EN via ?lang=en
    const langParam = url.searchParams.get('lang');
    const lang = (langParam === 'en' ? 'en' : 'zh') as 'en' | 'zh';
    const planId = url.searchParams.get('plan');
    const showNav = url.searchParams.get('nav') === '1';

    // --- POST /api/edit — inline editor write ---
    if (request.method === 'POST' && url.pathname === '/api/edit') {
      if (!env.ADMIN_TOKEN) {
        return Response.json({ error: 'Admin not configured' }, { status: 403 });
      }
      let body: EditRequest;
      try {
        body = await request.json() as EditRequest;
      } catch {
        return Response.json({ error: 'Invalid JSON' }, { status: 400 });
      }
      if (!body.token || !(await timingSafeEqual(body.token, env.ADMIN_TOKEN))) {
        return Response.json({ error: 'Unauthorized' }, { status: 403 });
      }
      if (!body.table || !body.plan_id || !body.destination || !body.field || body.value === undefined) {
        return Response.json({ error: 'Missing required fields' }, { status: 400 });
      }
      try {
        if (body.table === 'days') {
          await updateDayField(env, body.plan_id, body.destination, body.day_number, body.field, body.value);
        } else if (body.table === 'timesofday') {
          if (!body.session_type) {
            return Response.json({ error: 'session_type required for timesofday' }, { status: 400 });
          }
          await updateSessionField(env, body.plan_id, body.destination, body.day_number, body.session_type, body.field, body.value);
        } else {
          return Response.json({ error: 'Invalid table' }, { status: 400 });
        }
        return Response.json({ ok: true });
      } catch (err) {
        return Response.json({ error: err instanceof Error ? err.message : 'Write failed' }, { status: 500 });
      }
    }

    // Service Worker
    if (url.pathname === '/sw.js') {
      const sw = `const C='trip-dashboard-v1';
self.addEventListener('install',()=>self.skipWaiting());
self.addEventListener('activate',e=>e.waitUntil(self.clients.claim()));
self.addEventListener('fetch',e=>{
  if(e.request.method!=='GET')return;
  e.respondWith(
    fetch(e.request).then(r=>{
      if(r.ok){caches.open(C).then(c=>c.put(e.request,r.clone()));}
      return r;
    }).catch(()=>caches.match(e.request).then(r=>r||new Response('Offline — please visit while online to cache this page',{status:503,headers:{'Content-Type':'text/plain'}})))
  );
});`;
      return new Response(sw, {
        headers: {
          'Content-Type': 'application/javascript',
          'Cache-Control': 'no-cache',
        },
      });
    }

    // Favicon — inline SVG airplane emoji
    if (url.pathname === '/favicon.ico') {
      const svg = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><text y=".9em" font-size="90">✈️</text></svg>';
      return new Response(svg, {
        headers: {
          'Content-Type': 'image/svg+xml',
          'Cache-Control': 'public, max-age=86400',
        },
      });
    }

    // API route: raw JSON
    if (url.pathname.startsWith('/api/plan/')) {
      const id = url.pathname.replace('/api/plan/', '');
      if (!id) {
        return Response.json({ error: 'Plan ID required' }, { status: 400 });
      }
      try {
        const data = await getPlan(env, id);
        if (!data) {
          return Response.json({ error: 'Plan not found' }, { status: 404 });
        }
        return Response.json({
          plan: data.plan,
          updated_at: data.updated_at,
        }, {
          headers: { 'Cache-Control': 'public, max-age=60' },
        });
      } catch (err) {
        return Response.json(
          { error: err instanceof Error ? err.message : 'Unknown error' },
          { status: 500 }
        );
      }
    }

    // Dashboard route
    if (url.pathname === '/' || url.pathname === '') {
      // No plan specified → show plan index
      if (!planId) {
        try {
          const plans = await listPlans(env);
          const html = renderPlanIndex(plans, lang);
          return new Response(html, {
            headers: {
              'Content-Type': 'text/html; charset=utf-8',
              'Cache-Control': 'public, max-age=60',
            },
          });
        } catch (err) {
          const msg = err instanceof Error ? err.message : 'Unknown error';
          return new Response(renderError(`Database error: ${msg}`, lang), {
            status: 502,
            headers: { 'Content-Type': 'text/html; charset=utf-8' },
          });
        }
      }

      // Check edit mode: ?edit=TOKEN
      const editToken = url.searchParams.get('edit');
      const editMode = !!(editToken && env.ADMIN_TOKEN && await timingSafeEqual(editToken, env.ADMIN_TOKEN));

      try {
        const [planData, plans] = await Promise.all([
          getDashboardPlan(env, planId),
          showNav ? listPlans(env) : Promise.resolve(undefined),
        ]);
        if (!planData) {
          return new Response(renderError(`Plan "${planId}" not found`, lang), {
            status: 404,
            headers: { 'Content-Type': 'text/html; charset=utf-8' },
          });
        }

        const activeDest = planData.plan.active_destination as string;
        const bookings = await getBookings(env, activeDest);

        const html = renderDashboard(planData, bookings, lang, planId, plans, env.GOOGLE_MAPS_KEY, editMode);
        return new Response(html, {
          headers: {
            'Content-Type': 'text/html; charset=utf-8',
            'Cache-Control': 'public, max-age=60',
          },
        });
      } catch (err) {
        const msg = err instanceof Error ? err.message : 'Unknown error';
        return new Response(renderError(`Database error: ${msg}`, lang), {
          status: 502,
          headers: { 'Content-Type': 'text/html; charset=utf-8' },
        });
      }
    }

    // 404 for anything else
    return new Response('Not found', { status: 404 });
  },
} satisfies ExportedHandler<Env>;
