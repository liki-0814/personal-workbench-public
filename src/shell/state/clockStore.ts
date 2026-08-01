import { useState, useEffect, useRef } from 'react';
import { getDayOfWeek } from '@/core/utils/date';

interface ClockData {
  time: string;
  date: string;
  weekday: string;
}

function formatTime(now: Date) {
  return now.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false });
}

function formatDate(now: Date) {
  return `${now.getFullYear()}年${now.getMonth() + 1}月${now.getDate()}日`;
}

export function useClock() {
  const [clock, setClock] = useState<ClockData>(() => {
    const now = new Date();
    return { time: formatTime(now), date: formatDate(now), weekday: getDayOfWeek(now) };
  });
  const lastDateRef = useRef(clock.date);

  useEffect(() => {
    const timer = setInterval(() => {
      const now = new Date();
      const time = formatTime(now);
      const date = formatDate(now);
      if (date !== lastDateRef.current) {
        lastDateRef.current = date;
        setClock({ time, date, weekday: getDayOfWeek(now) });
      } else {
        setClock(prev => prev.time === time ? prev : { ...prev, time });
      }
    }, 1000);
    return () => clearInterval(timer);
  }, []);

  return clock;
}
