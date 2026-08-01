import { useEffect, useRef } from 'react';
import type { DayPlanItem } from '../types';
import { minuteOfDay } from '../utils';

export function useDayPlanReminders(plans: DayPlanItem[]) {
  const notified = useRef(new Set<string>());

  useEffect(() => {
    if (typeof window === 'undefined' || !('Notification' in window)) return;
    if (Notification.permission === 'default') Notification.requestPermission().catch(() => {});
  }, []);

  useEffect(() => {
    if (typeof window === 'undefined') return;
    const check = () => {
      if (document.hidden || !('Notification' in window) || Notification.permission !== 'granted') return;
      const currentMinute = minuteOfDay();
      plans.forEach(plan => {
        if (currentMinute < plan.startMinute || notified.current.has(plan.id)) return;
        notified.current.add(plan.id);
        new Notification('日内计划开始了', { body: plan.title, icon: '/pwcli.svg', tag: `day-plan-${plan.id}` });
      });
    };
    check();
    const timer = window.setInterval(check, 30_000);
    return () => window.clearInterval(timer);
  }, [plans]);
}
