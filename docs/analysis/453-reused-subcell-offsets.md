# 453 — Переиспользование subcell offsets

3 ноября 2026, ADR-827. Артефакты: `out/optimization-453/` и nuclear CSV/hashes
`out/optimization-443/{before,after}-453.*`.

## Причина

`read_contacts_inner`, полный contact step и ground-only step создавали
`subcell_offsets(edge)` на каждый вызов. При `edge=64` это 262 144 записей
`[i32; 3]`, около 3 МиБ и столько же делений/остатков на одну генерацию. Затем массив
удалялся и создавался заново при следующем чтении той же волны.

Это не lazy arithmetic и не material summary из отклонённого 443: массив, layout и
порядок доступа сохраняются. Изменяется только время его жизни.

## ADR-827

`Explosion` владеет derived `offsets: Vec<[i32; 3]>`:

- `begin` создаёт массив один раз после определения `cell`;
- три горячих пути временно берут его через `take` и возвращают при каждом выходе
  через общий closure, включая deferred claims и ранние return;
- snapshot buffer не содержит, `load` заново строит его из сохранённого `cell`;
- contact order, material payload, cursors, energy, quotas, IDs, ledger, save bytes и
  hash не меняются.

Новых зависимостей и persistent cross-wave cache нет.

## Проверки

- 19 contact-read тестов прошли: uniform/mixed payloads, wrapped/deferred contacts,
  occupancy, live fallback, energy-limited prefixes и wide material IDs.
- Две ordinary ABBA серии по1600 тиков: все35 нетайминговых CSV-полей каждого тика
  совпали в обеих сериях.
- Replay snapshot300→600 на physics1/4/8: `695327e0588d053c`; snapshot bytes совпали.
- Nuclear: все181 hash совпали, ledger сбалансирован, hash `ae3798a582873796`.

## Ordinary AI-vs-AI

physics4/navigation8/Rayon4, одинаковый мир и flags:

| Пара | Mean | Позднее окно1201–1600 |
|---|---:|---:|
|1|32,499→32,238 мс (−0,80%)|45,937→45,765 мс (−0,37%)|
|2|32,291→32,042 мс (−0,77%)|45,813→45,289 мс (−1,14%)|

Сводно mean32,395→32,140 мс, **−0,79%**; позднее окно45,875→45,527 мс,
**−0,76%**. Улучшение присутствует в обеих сериях, но оно небольшое и не является
новым native FPS-результатом или решением retained-history роста.

Baseline: `out/optimization-452/restored.exe`, SHA-256
`51f80d54940574c3bb32fe572fd3afb5f672a70a31a7f18dd9ade3fd6ad29c98`.
Candidate: `out/optimization-453/offsets-reused.exe`, SHA-256
`0cc3e1981cf6f4cb63160b7562c85f0a6bcdf51fc45937024e7e227e2e373cd8`.

## Nuclear edge=64

Тот же `out/nuclear-380/before-impact.snap`,181 тик,physics4/navigation8/Rayon4.
Это вспомогательное сравнение с frozen nuclear probe, а не замена строгого ordinary
ABBA.

| Показатель | Before | After | Изменение |
|---|---:|---:|---:|
|Mean tick (лог probe)|19,750 мс|17,548 мс|−11,15%|
|Худший тик|40,534 мс|38,080 мс|−6,05%|
|Суммарный `read_ground` scope|349,924 мс|173,537 мс|−50,41%|
|Суммарный `read_coarse` scope|173,996 мс|166,162 мс|−4,50%|
|Суммарный `read_pack` scope|32,811 мс|31,788 мс|−3,12%|

Nuclear демонстрирует цену удаления повторного buffer на большом `edge³`. Числа не
переносятся как такой же процент на обычный бой или FPS.

## Границы

Retained Settled/wake/rubble, exact paid claims/capture, owned publication и общая
FPS-приёмка остаются открытыми. Это точное устранение повторной подготовки одного
буфера, а не решение всей OPT316-02b или заявленное ускорение каждого сценария.
