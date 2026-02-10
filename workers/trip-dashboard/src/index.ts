import type { Env } from './turso';
import { getPlan, getBookings, listPlans } from './turso';
import { renderDashboard, renderError } from './render';

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    // ZH is default; switch to EN via ?lang=en
    const langParam = url.searchParams.get('lang');
    const lang = (langParam === 'en' ? 'en' : 'zh') as 'en' | 'zh';
    const planId = url.searchParams.get('plan');
    const showNav = url.searchParams.get('nav') === '1';

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
          plan: JSON.parse(data.plan_json),
          state: data.state_json ? JSON.parse(data.state_json) : null,
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
      if (!planId) {
        return new Response(renderError(
          lang === 'zh'
            ? '請聯繫旅行計畫擁有者取得行程連結'
            : 'Please contact the trip owner for a valid plan link',
          lang
        ), {
          status: 403,
          headers: { 'Content-Type': 'text/html; charset=utf-8' },
        });
      }

      try {
        const [planData, plans] = await Promise.all([
          getPlan(env, planId),
          showNav ? listPlans(env) : Promise.resolve(undefined),
        ]);
        if (!planData) {
          return new Response(renderError(`Plan "${planId}" not found`, lang), {
            status: 404,
            headers: { 'Content-Type': 'text/html; charset=utf-8' },
          });
        }

        const plan = JSON.parse(planData.plan_json);
        const activeDest = plan.active_destination as string;
        const bookings = await getBookings(env, activeDest);

        const html = renderDashboard(planData, bookings, lang, planId, plans);
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
