# The J49 in Home Assistant

`j49-mqtt` announces one device, "J49 thermostat <last four of the MAC>", by MQTT discovery (topics and their meaning:
`thermostat.md`). This file has what does not come by itself: cards for the dashboard.

The names of the entities follow the language of the device (`mqtt.*` in `tools/lang`); below are
the English ones. Entity ids are made by Home Assistant from the names when it first sees the
device, so yours may differ.

The device page in Home Assistant has a link "Visit" to the web page of the thermostat
(`configuration_url`, the address the device has when it connects to the broker).

## The week schedule

Entities:

- `Week schedule` (sensor): its state is the number of time points; its **attributes** are the week,
  `mon` .. `sun`, each a list of `{"time": "06:30", "temp": 20.0}`. They come from the topic
  `j49/schedule/week`.
- `Schedule Monday` .. `Schedule Sunday` (text): one day as text, `06:30 20.0, 08:30 17.0, 22:30
  16.0`. As many time points as you like, up to 12 a day; an empty text is a day without changes.
  Editing a field changes that day on the device. A text that cannot be read is refused and the
  field shows the old value again.
- `Schedule` (switch): follow the schedule or not.

To change several days at once, from an automation or a script, publish JSON to
`j49/schedule/week/set`; days that are not named keep what they have:

```yaml
action: mqtt.publish
data:
  topic: j49/schedule/week/set
  qos: 1
  payload: >-
    {"sat": [{"time": "08:00", "temp": 20.5}, {"time": "23:00", "temp": 16}],
     "sun": [{"time": "09:00", "temp": 20.5}, {"time": "23:00", "temp": 16}]}
```

A day may also be written as pairs, `[["08:00", 20.5], ["23:00", 16]]`, or as the day text.

### A card that shows the week

A Markdown card, no extra components. Not tried in Home Assistant yet; the entity id is the one
Home Assistant derives from the names and may differ on your system.

```yaml
type: markdown
title: Week schedule
content: |
  {% set s = 'sensor.j49_thermostat_week_schedule' %}
  {% set days = [('mon','Monday'),('tue','Tuesday'),('wed','Wednesday'),('thu','Thursday'),('fri','Friday'),('sat','Saturday'),('sun','Sunday')] %}
  | Day | Time and temperature |
  |:---|:---|
  {% for k, name in days -%}
  | {{ name }} | {% for p in state_attr(s, k) or [] %}**{{ p.time }}** {{ p.temp }}°{{ ' · ' if not loop.last }}{% endfor %} |
  {% endfor %}
```

### A card to edit it

```yaml
type: entities
title: Edit the schedule
entities:
  - switch.j49_thermostat_schedule
  - text.j49_thermostat_schedule_monday
  - text.j49_thermostat_schedule_tuesday
  - text.j49_thermostat_schedule_wednesday
  - text.j49_thermostat_schedule_thursday
  - text.j49_thermostat_schedule_friday
  - text.j49_thermostat_schedule_saturday
  - text.j49_thermostat_schedule_sunday
```

A graphical editor inside Home Assistant (dragging blocks in a week grid) needs a custom card; the
web page of the device (`http://<device>/`) has an editor with rows per time point.

## Other things worth a button

- `Away` (switch) and the preset "away" of the climate entity are the same setting.
- `Tune` (select) plays a tune; `Tune (RTTTL)` takes a melody of your own. (A screen program that can speak adds a text field `Say`.)
- `Sleep allowed`, `Sleep state`, `Last woken by`: see `sleep-j49.md`.
