import type { Env } from './turso';
import { getPlan, getBookings } from './turso';
import { renderDashboard, renderError } from './render';

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    const lang = (url.searchParams.get('lang') === 'zh' ? 'zh' : 'en') as 'en' | 'zh';
    const planId = url.searchParams.get('plan') || env.DEFAULT_PLAN_ID;

    // API route: raw JSON
    if (url.pathname.startsWith('/api/plan/')) {
      const id = url.pathname.replace('/api/plan/', '') || planId;
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
      try {
        const planData = await getPlan(env, planId);
        if (!planData) {
          return new Response(renderError(`Plan "${planId}" not found`, lang), {
            status: 404,
            headers: { 'Content-Type': 'text/html; charset=utf-8' },
          });
        }

        const plan = JSON.parse(planData.plan_json);
        const activeDest = plan.active_destination as string;
        const bookings = await getBookings(env, activeDest);

        const html = renderDashboard(planData, bookings, lang);
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
