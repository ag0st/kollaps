#[cfg(test)]
mod tests {
    use matrix::SymMatrix;

    #[derive(Copy, Clone)]
    struct DataTest(bool);

    impl DataTest {
        fn new(_: usize, _: usize) -> Self {
            DataTest(false)
        }
        fn set(&mut self, val: bool) {
            self.0 = val;
        }
    }

    fn create_matrix_with(size: usize, val: i32) -> SymMatrix<i32> {
        SymMatrix::new_fn(size, |_, _| val)
    }

    #[test]
    fn create_matrix() {
        let m1 = create_matrix_with(4, 4);
        assert_eq!(m1.size(), 4);
        for i in 0..m1.size() {
            for j in 0..m1.size() {
                assert_eq!(m1[(i, j)], 4)
            }
        }
    }

    #[test]
    fn grow_matrix() {
        let ori_val = 4;
        let new_val = 3;
        if ori_val == new_val {
            panic!("Must not have the same value")
        }
        let m1 = create_matrix_with(4, ori_val);
        let m2 = m1.grow_fn(2, |_, _| new_val);
        for i in 0..m2.size() {
            for j in 0..m2.size() {
                if i >= m1.size() || j >= m1.size() {
                    assert_eq!(m2[(i, j)], new_val)
                } else {
                    assert_eq!(m2[(i, j)], ori_val)
                }
            }
        }
    }

    #[test]
    fn modify_matrix() {
        let mut m1 = SymMatrix::new_fn(4, DataTest::new);
        m1[(2, 2)].set(true);
        assert_eq!(m1[(2, 2)].0, true)
    }

    #[test]
    fn remove_id() {
        let mut val = 0;
        let m1 = SymMatrix::new_fn(4, |_ ,_| {
            val += 1;
            val
        });
        println!("Original {:?}", m1);

        let m2 = m1.remove_id(1);
        println!("Second {:?}", m2);
    }
}