# Logos RFP-022 — исследовательский reverse-settlement prototype

**Текущий результат:** исполняемый burn → native LEZ acceptance и экспериментальный checkpoint guest ограниченного перехода состояния. Четыре native PPE receipts получены; outer checkpoint proving на CPU ещё выполняется, его receipt пока не получен. Публикация того же блока аутентифицируется настоящим channel verifier Logos. **Trustless release на Ethereum не реализован:** доказательство канонической финализированной source history отсутствует, consumer закрыт.

Это отдельная локальная работа. CRM, заявка, Google Doc, чужие ветки и исходники не изменялись. Ничего не опубликовано и не задеплоено.

## Одна команда

Из этой директории:

```sh
./reproduce.sh
```

Команда проверяет/клонирует закреплённые native репозитории в `../../work/upstream`, устанавливает локальный RISC Zero toolchain, собирает guests/host и заново производит настоящие proofs. Нужны Rustup, Git, curl, rg и инструменты сборки native Rust/C++ зависимостей. Проверено на macOS arm64; Linux x86_64 предусмотрен bootstrap-скриптом, но здесь не проверялся. Сохраняйте структуру `outputs/reverse-settlement/` при распаковке архива.

Для повторного исполнения уже полученных PPE receipts: `REUSE_PROOFS=1 ./reproduce.sh`. Они каждый раз криптографически проверяются, подписываются с актуальными payer nonces и проходят native execution заново. Это не измерение нового proving. Checkpoint receipt по умолчанию генерируется заново. Для быстрой повторной проверки всего сохранённого корпуса в той же сборке: `REUSE_PROOFS=1 REUSE_CHECKPOINT=1 ./reproduce.sh`. Это повторная криптографическая проверка, а не новое proving; несовпадение image IDs или исходного состояния приводит к отказу.

## Что исполняется

- Собственная минимальная LEZ-программа `burn-core`: 100 синтетических wrapped units в genesis; transfer в приватный account; burn уменьшает баланс и отправляет amount-bound exit commitment в публичный PDA accumulator. Ограничение accumulator одним выходом введено специально для воспроизводимого stale-state revert.
- Настоящие native PPE proofs на RISC Zero **3.1.0**, `r0vm` **3.1.0**, исправленные circuits **4.1.0**. Dev mode выключен, Fake receipts отклоняются. Старые 3.0.5/rc3 артефакты не используются.
- Полный штатный `chain_state::apply_block`: fee reserve/action/refund, receipts, обязательные fee/clock transactions, hash/link validation. Caller применяет блок на scratch clone и коммитит только после `Ok`.
- `acceptance-core` — экспериментальный executable reference для одного private action. Native Merkle tree и его сериализация взяты из закреплённого кода без изменений; dispatch использует штатный `lee_core::PublicPart`. Результат сверяется **побайтно по полному V03State** со штатными `ValidatedStateDiff` и `apply_state_diff`.
- Checkpoint guest через `env::verify` проверяет native PPE receipt, live code identity, signature/nonces, commitment freshness, known roots и nullifiers, исполняет публичную часть и коммитит previous/next full-state digest, exit root и native transaction hash. Prover получает публичное состояние/receipt; пользовательские ask/nsk и plaintext private balance ему не передаются.
- Native Logos `InscriptionOp`/`SignedOperation`: подпись над настоящим transaction signing hash, parent, accredited key и slot rotation; native body root связывает bytes принятого LEZ-блока с transaction body. Channel genesis/конфигурация — синтетические fixtures. Ledger validity, PoL и финальность этим не доказаны.

## Проверяемые отрицательные случаи

Смотрите фактический вывод в `evidence/settlement.log`, `evidence/native-auth.log`, `evidence/checkpoint.log` и `evidence/gate-tests.log`:

1. Два настоящих proofs для разных burns из одного private predecessor. После первого принятого burn второй получает `Nullifier already seen`, даже после обновления payer nonce/signature.
2. Настоящий private proof против пустого simulated sink; live sink уже заполнен. Native settlement возвращает `success=false`, нового exit нет.
3. Принудительный `success=true` и заново вычисленный корректный block hash не обходят native receipt comparison. Изменения scratch state не коммитятся.
4. Receipts входят в LEZ block hash. Изменение `BedrockStatus` на `Finalized` не меняет hash и ничего не доказывает.
5. Изменение inscription payload ломает подпись и меняет message ID/transaction hash/body root; wrong parent/key/slot отклоняются.
6. В checkpoint runner реализованы проверки journal/image ID/predecessor, replay, spent nullifier и неизвестного root. Полный прогон этих проверок ожидает outer receipt; положительное выполнение guest и отдельный Fake-receipt test уже выполнены.

## Фактические измерения

Apple M1 Pro, 10 logical CPUs, 32 GiB RAM; host в dev profile, официальный release `r0vm 3.1.0`. Запуски шли с параллельной нагрузкой. Это end-to-end время генерации/проверки конкретного PPE receipt, не throughput сети.

| Реальный native PPE proof | Время, с | Размер proof, байт |
|---|---:|---:|
| Приватное funding/shield | 354,160 | 223 047 |
| Принятый burn | 457,415 | 223 047 |
| Конфликтующий burn | 887,824 | 223 047 |
| Burn со stale public simulation | 833,473 | 223 047 |

Для checkpoint вход составил 1 229 647 байт. Исполнение исходного варианта потребовало 208 006 046 user cycles; профиль отнёс около 83% к serde/input чтению. Штатный LEZ framing снизил это до **26 574 665 user cycles**, 38 segments; execution-only run занял 4,046 с. Native crypto-bigint patch сам по себе cycle count не изменил. Медленный baseline proving остановлен до получения receipt и не считается доказательством.

После завершения checkpoint proving его verification и RSS будут записаны в `evidence/checkpoint.json` и `evidence/metrics.json`; на момент этого коммита outer receipt ещё отсутствует. gas и полная source history не измерены. `initial-generation-time.log` сохраняет первую ошибку обвязки: fee reserve уже изменял scratch state, а harness ошибочно ожидал rollback от низкоуровневого `settle_transaction`. Проверка перенесена на native `apply_block` с commit-on-Ok; полный исправленный прогон прошёл. Три уже полученных настоящих receipts были повторно проверены; четвёртый proof сгенерирован после исправления.

Ожидаемые guest panics в stderr относятся к отрицательным сценариям; критерий успешного прогона — exit code 0 и PASS assertions host harness.

## Точная граница результата

Checkpoint — **proof ограниченного native action, а не всего LEZ block settlement**. Fee/clock, общие программы, upgrades, async/recovery и источники history roots в этом guest не доказаны. Полный блок проверен отдельно native execution, что не превращает этот test в Ethereum-verifiable proof.

Начальное состояние выведено native кодом из нашего синтетического genesis. Это не production initialization и не аутентификация реальной Logos history. Previous digest должен непрерывно следовать из ранее проверенного checkpoint; оператор не может просто предложить root как новый anchor. Сейчас у consumer вообще нет source-finality verifier, поэтому release отклоняется и после валидного action proof.

В Logos PoL public inputs зависят от aged/latest UTXO roots и epoch state **исполненной** истории. Проверка одной PoL и выбранной глубины не заменяет replay, bootstrap/fork choice или каноничность ветки. LIB RPC и подпись штатных sequencer keys этого не закрывают. Native bootstrap с trusted checkpoint provider добавляет доверенного участника; genesis replay не доказывает, что prover наблюдал все значимые конкурирующие ветки. Это требует явного решения с Logos, включая временные/сетевые предположения.

Приватный user proof membership/секрета/destination/nullifier ещё не создан. Здесь deterministic test keys, synthetic binding bytes и canonical sender presentation; privacy/linkability гарантии RFP не заявляются. На Ethereum proof не проверялся, Groth16 conversion и gas не измерялись.

## Что нужно от Logos и бюджет

Нужны стабильные genesis/era/replay APIs и test histories; правило аутентифицируемой финальности с ясным bootstrap/canonicality; acceptance checkpoint на реальном commit point LEZ с полным state/code/nullifier/root-history commitment. Это можно строить как off-chain zk replay; добавление непроверяемого root в header не решает задачу. Native Zone STF enforcement остаётся незавершённым protocol work.

**Около $80k на RFP-022 имеет смысл планировать условно**, если Logos предоставляет или совместно закрывает этот primitive, а scope команды — его потребление, burn/release binding и отдельная privacy схема. Новый полный consensus zk light client, canonicality/bootstrap protocol и весь private bridge нельзя честно включить в эту сумму как уже оценённую работу. Независимый аудит сюда не входит. Следующий полезный milestone — согласованный source-history interface и воспроизводимый proof реального окна истории; только затем фиксировать основной implementation scope.

Подробности: [граница и интерфейс](docs/INTERFACE.md), [первичные источники](docs/SOURCES.md), версии — [pins.json](evidence/source/pins.json). Реальные измерения и результаты лежат в `evidence/`; полная source-chain стоимость по ним не экстраполируется.
