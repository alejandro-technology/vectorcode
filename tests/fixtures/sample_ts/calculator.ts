export class Calculator {
  private value: number;

  constructor(initial: number = 0) {
    this.value = initial;
  }

  add(n: number): Calculator {
    this.value += n;
    return this;
  }

  subtract(n: number): Calculator {
    this.value -= n;
    return this;
  }

  multiply(n: number): Calculator {
    this.value *= n;
    return this;
  }

  getResult(): number {
    return this.value;
  }

  reset(): void {
    this.value = 0;
  }
}

export function createCalculator(initial?: number): Calculator {
  return new Calculator(initial);
}

export interface CalculatorOptions {
  initialValue?: number;
  precision?: number;
}
