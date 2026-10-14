#!/usr/bin/env python3
"""Rebuild data/*.csv.gz from the public sources. Standard library only.

    python3 scripts/fetch_data.py            # downloads ~25 MB, writes data/

Each output file is `timestamp,value`: Unix seconds, then the value exactly as
the source printed it. Rows the source marks as missing are dropped, so the
timestamps have real gaps. data/README.md says where each file comes from and
under what licence.
"""
import calendar, csv, datetime as dt, gzip, io, json, os, sys, urllib.request, zipfile

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "data")
UA = {"User-Agent": "tsz-bench (https://github.com/ahmed-hashim-pro/tsz)"}


def get(url):
    with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=300) as r:
        return r.read()


def write(name, rows):
    path = os.path.join(OUT, name)
    # mtime=0 so the same rows always give the same bytes.
    with open(path, "wb") as raw, gzip.GzipFile(fileobj=raw, mode="wb", mtime=0, compresslevel=9) as gz:
        gz.write("".join(f"{t},{v}\n" for t, v in rows).encode())
    print(f"{name}: {len(rows)} points")


def utc(s, fmt):
    return calendar.timegm(dt.datetime.strptime(s, fmt).timetuple())


def noaa(product, column, extra=""):
    rows = []
    for m in range(1, 13):
        last = calendar.monthrange(2024, m)[1]
        url = ("https://api.tidesandcurrents.noaa.gov/api/prod/datagetter"
               f"?begin_date=2024{m:02}01&end_date=2024{m:02}{last}&station=9414290"
               f"&product={product}{extra}&units=metric&time_zone=gmt&format=csv&application=tsz-bench")
        for rec in list(csv.reader(io.StringIO(get(url).decode())))[1:]:
            if rec and rec[column].strip():
                rows.append((utc(rec[0], "%Y-%m-%d %H:%M"), rec[column].strip()))
    return rows


def main():
    os.makedirs(OUT, exist_ok=True)
    write("noaa-sf-water-level.csv.gz", noaa("water_level", 1, "&datum=MLLW"))
    write("noaa-sf-air-temp.csv.gz", noaa("air_temperature", 1))
    write("noaa-sf-wind-speed.csv.gz", noaa("wind", 1))

    url = ("https://wikimedia.org/api/rest_v1/metrics/pageviews/aggregate/"
           "en.wikipedia/all-access/user/hourly/2024010100/2024123123")
    items = json.loads(get(url))["items"]
    write("wikipedia-hourly-views.csv.gz",
          [(utc(i["timestamp"], "%Y%m%d%H"), str(i["views"])) for i in items])

    z = zipfile.ZipFile(io.BytesIO(get(
        "https://archive.ics.uci.edu/static/public/235/individual+household+electric+power+consumption.zip")))
    power, volts = [], []
    with z.open("household_power_consumption.txt") as f:
        for line in io.TextIOWrapper(f):
            p = line.rstrip("\n").split(";")
            if not p[0][:1].isdigit() or not p[0].endswith("/2007") or p[2] == "?":
                continue
            t = utc(p[0] + " " + p[1], "%d/%m/%Y %H:%M:%S")
            power.append((t, p[2]))
            volts.append((t, p[4]))
    write("uci-power-active-2007.csv.gz", power)
    write("uci-power-voltage-2007.csv.gz", volts)


if __name__ == "__main__":
    sys.exit(main())
