# Chalybs Pipeline – Execution Flow

This describes the complete deterministic bring‑up sequence.

## Mermaid Diagram

```mermaid
flowchart TD
    Init --> Validate
    Validate --> ReserveCpus
    ReserveCpus --> LaunchQemu
    LaunchQemu --> DetectThreads
    DetectThreads --> PinVcpus
    PinVcpus --> DetectMsi
    DetectMsi --> PinIrqs
    PinIrqs --> PeripheralHooks
    PeripheralHooks --> SteadyState
```

## ASCII Diagram

```
+-----------+     +-----------+     +--------------+     +--------------+
|   Init    | --> | Validate  | --> | ReserveCpus  | --> |  LaunchQemu  |
+-----------+     +-----------+     +--------------+     +--------------+
                                                             |
                                                             v
                                                    +-------------------+
                                                    |  DetectThreads   |
                                                    +-------------------+
                                                             |
                                                             v
                                                    +-------------------+
                                                    |    PinVcpus      |
                                                    +-------------------+
                                                             |
                                                             v
                                                    +-------------------+
                                                    |    DetectMsi     |
                                                    +-------------------+
                                                             |
                                                             v
                                                    +-------------------+
                                                    |     PinIrqs      |
                                                    +-------------------+
                                                             |
                                                             v
                                                    +-------------------+
                                                    | PeripheralHooks  |
                                                    +-------------------+
                                                             |
                                                             v
                                                    +-------------------+
                                                    |   SteadyState    |
                                                    +-------------------+
```
