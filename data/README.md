# Benchmark data

Six real series, each `timestamp,value` with Unix seconds and the value
exactly as the source printed it, gzipped. `scripts/fetch_data.py` rebuilds
every file from its source; rows the source marks as missing are dropped, so
the gaps are real.

| File | Points | What | Source | Licence |
|---|--:|---|---|---|
| `noaa-sf-water-level.csv.gz` | 73,661 | Water level, metres above MLLW, every 6 minutes, 2024 | NOAA CO-OPS, station 9414290 (San Francisco), via the [CO-OPS API](https://api.tidesandcurrents.noaa.gov/api/prod/) | US Government work, public domain |
| `noaa-sf-air-temp.csv.gz` | 87,637 | Air temperature, °C, every 6 minutes, 2024 | as above | public domain |
| `noaa-sf-wind-speed.csv.gz` | 87,637 | Wind speed, m/s, every 6 minutes, 2024 | as above | public domain |
| `uci-power-active-2007.csv.gz` | 521,669 | One household's global active power, kW, every minute, 2007 | G. Hébrail and A. Bérard, *Individual Household Electric Power Consumption*, UCI Machine Learning Repository, 2006, [doi:10.24432/C58K54](https://doi.org/10.24432/C58K54) | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| `uci-power-voltage-2007.csv.gz` | 521,669 | The same household's voltage, V, every minute, 2007 | as above | CC BY 4.0 |
| `wikipedia-hourly-views.csv.gz` | 8,784 | English Wikipedia page views by users, per hour, 2024 | [Wikimedia pageviews API](https://wikimedia.org/api/rest_v1/) | [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/) |

The NOAA values are preliminary (not yet verified by NOAA) and are used here
only as realistic sensor data. The UCI files are a subset (calendar 2007, two
of the nine columns) of the published dataset, unchanged otherwise.
