import { useState, useEffect } from 'react';

export interface WeatherData {
  temp: string;
  feelsLike: string;
  condition: string;
  icon: string;
  city: string;
  code?: string;
  humidity?: number;
  windSpeed?: number;
}

export interface City {
  name: string;
  lat: number;
  lon: number;
}

// Open-Meteo weather code mapping
const weatherCodeMap: Record<number, { label: string; icon: string; code: string }> = {
  0: { label: '晴朗', icon: '☀️', code: 'clear' },
  1: { label: '多云', icon: '🌤️', code: 'partly-cloudy' },
  2: { label: '多云', icon: '⛅', code: 'partly-cloudy' },
  3: { label: '阴天', icon: '☁️', code: 'cloudy' },
  45: { label: '雾', icon: '🌫️', code: 'fog' },
  48: { label: '雾凇', icon: '🌫️', code: 'fog' },
  51: { label: '毛毛雨', icon: '🌦️', code: 'drizzle' },
  53: { label: '毛毛雨', icon: '🌦️', code: 'drizzle' },
  55: { label: '毛毛雨', icon: '🌦️', code: 'drizzle' },
  61: { label: '小雨', icon: '🌧️', code: 'rain' },
  63: { label: '中雨', icon: '🌧️', code: 'rain' },
  65: { label: '大雨', icon: '🌧️', code: 'rain' },
  71: { label: '小雪', icon: '🌨️', code: 'snow' },
  73: { label: '中雪', icon: '🌨️', code: 'snow' },
  75: { label: '大雪', icon: '🌨️', code: 'snow' },
  80: { label: '阵雨', icon: '🌦️', code: 'showers' },
  81: { label: '阵雨', icon: '🌦️', code: 'showers' },
  82: { label: '暴雨', icon: '⛈️', code: 'storm' },
  95: { label: '雷雨', icon: '⛈️', code: 'storm' },
  96: { label: '雷雨', icon: '⛈️', code: 'storm' },
  99: { label: '雷雨', icon: '⛈️', code: 'storm' },
};

const chineseCities: City[] = [
  { name: '北京', lat: 39.9042, lon: 116.4074 },
  { name: '上海', lat: 31.2304, lon: 121.4737 },
  { name: '广州', lat: 23.1291, lon: 113.2644 },
  { name: '深圳', lat: 22.5431, lon: 114.0579 },
  { name: '杭州', lat: 30.2741, lon: 120.1551 },
  { name: '成都', lat: 30.5728, lon: 104.0668 },
  { name: '西安', lat: 34.3416, lon: 108.9398 },
  { name: '武汉', lat: 30.5928, lon: 114.3055 },
  { name: '南京', lat: 32.0603, lon: 118.7969 },
  { name: '重庆', lat: 29.5630, lon: 106.5516 },
];

export function useWeather() {
  const [weather, setWeather] = useState<WeatherData | null>(null);
  const [loading, setLoading] = useState(true);
  const [selectedCity, setSelectedCity] = useState(chineseCities[0]);

  useEffect(() => {
    let cancelled = false;

    const fetchWeather = async () => {
      try {
        setLoading(true);

        // Use Open-Meteo API (free, no API key needed)
        const url = `https://api.open-meteo.com/v1/forecast?latitude=${selectedCity.lat}&longitude=${selectedCity.lon}&current=temperature_2m,relative_humidity_2m,apparent_temperature,weather_code,wind_speed_10m&timezone=auto`;

        const controller = new AbortController();
        const timeoutId = setTimeout(() => controller.abort(), 10000);

        const res = await fetch(url, { signal: controller.signal });
        clearTimeout(timeoutId);

        if (!res.ok) throw new Error('Weather fetch failed');

        const data = await res.json();
        const current = data.current;

        const weatherInfo = weatherCodeMap[current.weather_code] || { label: '未知', icon: '🌡️', code: 'unknown' };

        if (!cancelled) {
          setWeather({
            temp: `${Math.round(current.temperature_2m)}°C`,
            feelsLike: `${Math.round(current.apparent_temperature)}°C`,
            condition: weatherInfo.label,
            icon: weatherInfo.icon,
            city: selectedCity.name,
            code: weatherInfo.code,
            humidity: current.relative_humidity_2m,
            windSpeed: current.wind_speed_10m,
          });
        }
      } catch (err) {
        console.error('Error fetching weather:', err);
        if (!cancelled) {
          // Fallback data
          setWeather({
            temp: '--',
            feelsLike: '--',
            condition: '服务暂时不可用',
            icon: '🌡️',
            city: selectedCity.name,
            code: 'unknown',
          });
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    };

    fetchWeather();

    // Refresh every 30 minutes
    const interval = setInterval(fetchWeather, 30 * 60 * 1000);

    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [selectedCity]);

  return { weather, loading, cities: chineseCities, selectedCity, setSelectedCity };
}
