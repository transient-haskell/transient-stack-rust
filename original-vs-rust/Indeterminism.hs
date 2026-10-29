{-# LANGUAGE ScopedTypeVariables #-}
import Transient.Base
import Transient.Internals
import Transient.Indeterminism
import Control.Monad
import Control.Applicative
import Data.Typeable
import Control.Monad.IO.Class
import Control.Concurrent
import Control.Exception hiding (onException, catch)
import qualified System.Timeout as T
import Data.IORef

say :: String -> TransIO ()
say = liftIO . putStrLn

run :: String -> TransIO () -> IO ()
run name t = do
  putStrLn ("--- " ++ name)
  r <- T.timeout 3000000 (keep' t)
  case r of
    Nothing -> putStrLn "   (keep' returned nothing in 3 s)"
    Just rs -> putStrLn ("   keep' returns " ++ show (length rs) ++ " results")

main = do
  run "F1 for: the continuation runs in this thread, in order" $ do
    x <- for [1,2,3 :: Int]
    th <- liftIO myThreadId
    say ("for " ++ show x)

  run "F2 for <|> something else: the alternative runs after the loop" $ do
    (do x <- for [1,2 :: Int]; say ("a " ++ show x)) <|> say "alternative"

  run "F3 for with an asynchronous task inside: the loop continues" $ do
    x <- for [1,2,3 :: Int]
    async (threadDelay (1000 * (4 - x) * 10))
    say ("after async " ++ show x)

  run "C1 choose: one thread per element" $ do
    x <- choose [1,2,3 :: Int]
    liftIO $ threadDelay (x * 20000)
    say ("choose " ++ show x)

  run "C2 choose with threads 1: one at a time" $ do
    x <- threads 1 $ choose [1,2,3 :: Int]
    liftIO $ threadDelay 20000
    say ("choose1 " ++ show x)

  run "G1 group 2 de 5 eventos" $ do
    xs <- group 2 $ choose [1,2,3,4,5 :: Int]
    say ("group " ++ show xs)

  run "G2 group with a single thread in order (for)" $ do
    xs <- group 3 $ for [1,2,3,4,5,6 :: Int]
    say ("group " ++ show xs)

  run "S1 sync: waits for all the threads and gives the list" $ do
    rs <- sync $ choose [1,2,3 :: Int] >>= \x -> liftIO (threadDelay (x*10000)) >> return x
    say ("sync " ++ show (length rs))

  run "S2 sync of a computation without results: runs the alternative" $ do
    (do rs <- sync (async (return ()) >> empty :: TransIO ()); say ("sync " ++ show (length rs))) <|> say "alternative"

  run "S3 sync1: the first result" $ do
    x <- sync1 $ choose [3,1,2 :: Int] >>= \x -> liftIO (threadDelay (x*30000)) >> return x
    say ("sync1 " ++ show x)

  run "S4 timeout: arrives in time" $ do
    x <- timeout 500000 $ (async (threadDelay 10000 >> return (1 :: Int)))
    say ("timeout ok " ++ show x)

  run "S5 timeout: not reached" $ do
    (do x <- timeout 50000 $ (async (threadDelay 300000 >> return (1 :: Int))); say ("timeout ok " ++ show x))
      <|> say "timeout expired"

  run "O1 oneThread with a stream" $ do
    n <- liftIO $ newIORef (0 :: Int)
    x <- oneThread $ waitEvents (do threadDelay 20000; atomicModifyIORef n (\a -> (a+1, a+1)))
    when (x > 3) $ liftIO (return ()) >> empty
    async (threadDelay 50000)
    say ("process " ++ show x)

  run "B1 burst: SMore per result and SDone on going silent" $ do
    n <- liftIO $ newIORef (0 :: Int)
    r <- burst 100000 $ do
            x <- async (do threadDelay 20000; atomicModifyIORef n (\a -> (a+1, a+1)))
            return x
    say ("burst " ++ show (fmap id (r :: StreamData Int)))
    case r of SDone -> exit () ; _ -> return ()

  run "T1 groupByTime: groups what is received per interval" $ do
    n <- liftIO $ newIORef (0 :: Int)
    xs <- groupByTime 100000 $ do
            x <- waitEvents (do threadDelay 30000; atomicModifyIORef n (\a -> (a+1, a+1)))
            return [x]
    say ("batch " ++ show (xs :: [Int]))
